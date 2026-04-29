# Future Feature: Inference Delegation (Path 2 of GH9303)

**Status:** **Future / proposal-only**. Requires Warp backend + proto cooperation. NOT implemented.
**Filed against:** the upstream `warp-proto-apis` repo (proto extension) and Warp's backend team (server-side implementation).
**Origin:** [warpdotdev/warp#9303](https://github.com/warpdotdev/warp/issues/9303), proposed in-thread by `@tbitcs`.
**Companion:** [/specs/GH9303/](../../GH9303/) — *Path 1 (Custom Local LLM Provider)*, the contributor-buildable client-only spec being shipped now. Path 1 and Path 2 coexist; Path 2 does not displace Path 1.

## What this is

A second-generation answer to "let me use my own LLM with Warp" that **preserves Warp's tuned system prompt and tool JSON schemas** while keeping the LLM call itself on the user's own infrastructure. Path 1 (the spec being shipped) requires the OSS client to re-author the prompt and schemas — that's the source of its quality gap. Path 2 fixes that.

The mechanic: Warp's backend continues to do everything it does today (compose the system prompt, build tool schemas, run the agent loop, parse tool calls, format responses) **except** the actual LLM call. Instead of calling OpenAI / Anthropic / Bedrock from the server, the backend emits a new **`ClientAction::ExecuteLLMInference`** containing the fully-formulated provider-format payload. The client forwards that payload to the user's configured local endpoint, streams the model's response back to the server over a persistent channel, and the server resumes its existing pipeline as if the call had been local to the server.

## Why it's worth filing

| Aspect | Path 1 (shipped) | Path 2 (this proposal) |
|---|---|---|
| Quality of the system prompt | Hand-authored generic | Warp's continuously-tuned prompt |
| Tool-schema fidelity | 5 of 33 tools, hand-authored | All tools, server-authoritative |
| Agent-loop parity | Best-effort | Full parity |
| Works offline | Yes | No (control plane needs warp.dev) |
| Conversation content sees warp.dev | No (LLM call) | Metadata yes; conversation content no (it goes to the user's endpoint, not the server) |
| Requires backend changes | No | **Yes** |
| Requires proto extension | No | **Yes** (one new `ClientAction` variant + maybe one new `Input` variant) |

The privacy story for Path 2 is nuanced: Warp's backend still composes the prompt (so it can prepend its system instructions) and forwards it to the client to send to the LLM. The conversation **content** — both the prompt and the model response — flows through the client to the user's endpoint, not through warp.dev. Warp's backend sees orchestration metadata (which tools to enable, which model id, conversation IDs) but never touches the LLM-formatted payload to/from the local model after delegation.

## Sketch of the proto change

In `apis/multi_agent/v1/response.proto`, extend the `ClientAction.action` oneof:

```protobuf
message ClientAction {
  oneof action {
    // ... existing 13 variants ...
    ExecuteLLMInference execute_llm_inference = 15;
  }

  // Tells the client to perform an LLM inference call against its
  // configured local endpoint and stream the response back. Used when
  // the user has selected a "local provider" model in settings.
  message ExecuteLLMInference {
    // A unique ID the client uses to correlate this inference with
    // the response stream it sends back.
    string inference_id = 1;

    // The wire format the request is encoded in.
    Format request_format = 2;

    // The fully-formulated request payload, opaque to the client.
    // Encoded per `request_format`.
    bytes request_payload = 3 [ (sensitive) = true ];

    // Where to send the inference. Either a hint ("user's configured local provider")
    // or an explicit URL override.
    InferenceTarget target = 4;

    // How long to wait for the inference to complete before timing out.
    int64 timeout_ms = 5;

    enum Format {
      FORMAT_OPENAI_CHAT_COMPLETIONS = 0;
      FORMAT_ANTHROPIC_MESSAGES = 1;
      // future: GOOGLE_GENERATE_CONTENT, BEDROCK_INVOKE_STREAM, etc.
    }

    message InferenceTarget {
      oneof target {
        UseConfiguredProvider use_configured_provider = 1;
        ExplicitEndpoint explicit_endpoint = 2;
      }

      message UseConfiguredProvider {}
      message ExplicitEndpoint {
        string base_url = 1;
        string api_key = 2 [ (sensitive) = true ];
      }
    }
  }
}
```

And in `request.proto`, extend `Input` with a corresponding completion message the client sends back:

```protobuf
message Input {
  oneof type {
    // ... existing variants ...
    LLMInferenceResult llm_inference_result = 18;
  }

  message LLMInferenceResult {
    string inference_id = 1;             // matches ExecuteLLMInference.inference_id

    oneof outcome {
      Streamed streamed = 2;             // success: response body bytes (raw provider response)
      Failed failed = 3;
      TimedOut timed_out = 4;
      UserCancelled user_cancelled = 5;
    }

    message Streamed {
      Format response_format = 1;        // mirrors request format
      bytes response_payload = 2 [ (sensitive) = true ];
    }
    message Failed {
      int32 http_status = 1;             // 0 if not HTTP-level
      string error_message = 2;
    }
    message TimedOut {}
    message UserCancelled {}
  }
}
```

Both messages are additive; existing clients ignore them; existing servers can negotiate via the `Settings.supports_inference_delegation` capability bit (one additional bool on `Request.Settings`).

## Sketch of the client change

When `Settings.supports_inference_delegation = true` is set on the request and the active model has a local-provider routing host, the server emits `ExecuteLLMInference` instead of calling the LLM itself. The client's controller adds one new action handler:

```rust
match action {
    Action::ExecuteLLMInference(req) => {
        let body = local_provider::run_inference(req.request_payload, target, http).await;
        server_api.send_input(LLMInferenceResult { inference_id: req.inference_id, outcome: body.into() }).await;
    }
    // ... existing handlers ...
}
```

The local-provider crate from Path 1 is reused — only the wire-format adapter is needed (the prompt, tool schemas, conversation walk, etc. are all server-side here). This is a much smaller client surface than Path 1.

## Open questions for the Warp team

1. **Is the privacy story sellable?** Conversation content stays on the user's machine; metadata + the request payload (which contains conversation content) transits warp.dev once per turn. Marketing this requires care: it's "your LLM never sees warp.dev" not "warp.dev never sees your prompts."
2. **Streaming back through warp.dev or through a peer channel?** The simplest implementation streams the local model's response back through the existing request → response pipe (the server is "waiting" on the inference). An optimization streams it through a separate WebSocket peer channel; latency-sensitive but adds complexity.
3. **Trust boundary for `ExplicitEndpoint`.** Should the server be allowed to direct the client to send to an arbitrary URL, or only to the user's pre-configured local provider? The `UseConfiguredProvider` variant is safer; `ExplicitEndpoint` is more flexible. v1 of Path 2 would ship only `UseConfiguredProvider`.
4. **Multiple in-flight inferences (parallel tool calls).** When the server runs N local-model calls in parallel for, e.g., subagents, each gets its own `inference_id`. The client tracks an in-flight map keyed by `inference_id`.
5. **Failure handling.** A failed inference returns `LLMInferenceResult::Failed` and the server gets to decide whether to retry, fall back to a Warp-hosted model, or surface the error. v1 of Path 2 surfaces the error and stops.

## Why not start with this?

- Requires Warp team buy-in for proto changes AND backend implementation. A community contributor cannot land any part of Path 2 without internal collaboration.
- Path 1 is shippable today, demonstrates the demand, and produces real signal on which tools the community actually wants exposed.
- Path 2 builds on the Path 1 plumbing: the local-provider crate, settings, picker entry, and the network audit infrastructure all carry over. Path 2 essentially adds a second action handler and removes the prompt/schema authoring burden.

## What this doc is for

A talking-point for the Warp team's eventual evaluation. **Not for landing as code.** When/if the Warp team is interested, this doc gets converted to a real `specs/GH<num>/{product,tech}.md` against a fresh issue.
