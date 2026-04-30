# Phase 8 Manual Smoke Checklist — Custom Local LLM Provider

Walk through this once per environment before promoting `FeatureFlag::LocalLlmProvider` from off → `DOGFOOD_FLAGS`. Each row should produce a green check or a filed bug. Run from this branch (`nmehta/local-llm-provider`).

## Prerequisites

```bash
# 1. Install protoc (proto compilation in build.rs)
brew install protobuf

# 2. Make sure Xcode + Metal toolchain are installed
sudo xcodebuild -license   # accept once
xcodebuild -downloadComponent MetalToolchain   # one-time

# 3. Build the workspace once to warm caches
cargo check -p warp -p ai
```

## 0 — Library tests

```bash
cargo test -p ai local_provider::                # 69 unit tests
cargo test -p ai --test local_provider_integration   # 7 integration tests
```

Both green. Total 76 / 76 expected.

## 1 — Smoke against your own endpoint (no Warp app yet)

The smoke binary at `crates/ai/examples/local_provider_smoke.rs` does an end-to-end round-trip without booting the Warp client. **This is the fastest way to confirm a target endpoint speaks the wire format we expect.**

| Endpoint | Command | Pass criteria |
|---|---|---|
| Ollama 0.4 (mac/linux) | `cargo run -p ai --example local_provider_smoke -- --base-url http://localhost:11434/v1 --model llama3.1 --query "Say hi in one word."` | Exits 0, prints assistant text, finish=Done |
| LM Studio 0.3 (mac) | same with `--base-url http://localhost:1234/v1 --model <whatever-is-loaded>` | Exits 0 |
| vLLM 0.6 (gpu box) | `--base-url http://gpu-host:8000/v1 --model meta-llama/Llama-3.1-70B-Instruct` | Exits 0 |
| llama.cpp `server` | `--base-url http://localhost:8080/v1 --model <gguf>` | Exits 0 |
| NVIDIA NIM (HTTPS) | `--base-url https://integrate.api.nvidia.com/v1 --model meta/llama-3.1-70b-instruct --api-key "$NVIDIA_API_KEY"` | Exits 0, header shows in mitmproxy as `Authorization: Bearer ...` |

If a row fails, attach the smoke binary's stderr/stdout to the bug.

## 2 — Tool-call exercise

Add `--with-tools` and ask the model to use a tool. Most local servers support tool calls but quality varies wildly across small models.

```bash
cargo run -p ai --example local_provider_smoke -- \
    --base-url http://localhost:11434/v1 \
    --model qwen2.5-coder:7b \
    --with-tools \
    --query "Use the read_files tool to read Cargo.toml at the repo root"
```

Pass criteria: output includes a `[tool_call name=read_files id=...]` line and the typed proto variant is `Some(...)` (the binary prints `typed=true`).

If `typed=false` you've found a model that emits malformed tool-call JSON — file it as a model-specific issue, not a bug in our adapter.

## 3 — Configure inside Warp

Now bring up the actual client.

```bash
# 1. Start Warp
cargo run

# 2. Find the settings TOML (Settings → Show settings on disk).
#    Typical mac path:
#    ~/Library/Application Support/dev.warp.Warp-Stable/user_preferences.toml

# 3. Add the runtime feature flag override
cat >> "$(find ~/Library -name user_preferences.toml -path '*Warp*' 2>/dev/null | head -1)" <<'EOF'

[runtime_feature_flags]
LocalLlmProvider = true

[agents.local_provider]
enabled = true
display_name = "Ollama"
base_url = "http://localhost:11434/v1"
model_id = "llama3.1"
supports_tools = true
EOF

# 4. Restart Warp.
```

Then:

| Check | Expected |
|---|---|
| Open Settings → AI | "Local Provider API Key (optional)" field appears below the existing OpenAI/Anthropic/Google fields |
| Open the model picker in Agent Mode | An entry like `Ollama: llama3.1` appears (may need to log in / refresh once for `on_server_update` to fire). The auxiliary description is `Custom local provider`. |
| Select the local model + send "What is 2+2?" | Reply streams in. |
| Check `ps aux \| grep cargo run` while sending | Warp client is the only process talking to your endpoint. |

## 4 — Network audit (mitmproxy)

Confirm zero traffic to `*.warp.dev` for the LLM call.

```bash
brew install mitmproxy
mitmweb --mode regular --listen-port 8888 &  # web UI on http://localhost:8081

# Tell Warp to proxy through mitmproxy (mac):
HTTPS_PROXY=http://127.0.0.1:8888 \
HTTP_PROXY=http://127.0.0.1:8888 \
SSL_CERT_FILE="$HOME/.mitmproxy/mitmproxy-ca-cert.pem" \
cargo run

# In Warp: select your local model, send a message.
```

Audit the mitmproxy log:

| Allowed | Expected to be present |
|---|---|
| `*.warp.dev` telemetry, version-check (non-AI) | ✅ may appear |
| `127.0.0.1` / `localhost` requests to your endpoint | ✅ should appear (the actual LLM call) |

| **Forbidden** | **MUST NOT be present** |
|---|---|
| `*.warp.dev/ai/multi-agent` POST | ❌ if seen, the dispatch fork didn't fire |
| `*.warp.dev/agent-mode-evals/*` | ❌ same |

## 5 — Edge-case behaviors

| Scenario | Set up | Expected |
|---|---|---|
| Endpoint down | Stop Ollama, send a turn | Conversation shows error toast / message; no crash |
| Bad API key | Set wrong key in settings, restart | Endpoint returns 401, conversation reports the failure |
| Disable mid-conversation | Toggle `local_provider_enabled = false` and refresh | Picker drops the entry; previously-selected local model falls back; toast informs the user |
| Tool-call from a model that can't tool-call | Use a base llama 7b or similar | Either no tool calls (text-only response) or the synthetic "tried to call X but arguments were unusable" message — never a crash |
| Cancellation mid-stream | Hit stop while text is streaming | Stream closes cleanly, partial output is preserved as the assistant turn (matches existing behavior) |

## 6 — Sign-off

Once every row above is green, file:

```bash
gh issue comment 9303 --body "Phase 8 dogfood smoke complete on $(uname -smr). Matrix attached: $(date -u +%Y-%m-%dT%H:%MZ)"
```

Then promote the flag:

1. Edit `crates/warp_features/src/lib.rs`
2. Add `FeatureFlag::LocalLlmProvider` to `DOGFOOD_FLAGS`
3. Open the promotion PR, link this checklist, request `oz-review`.

After ≥2 weeks of dogfood with no regressions, repeat for `PREVIEW_FLAGS`. Stable comes later.

## Common pitfalls

- **The picker doesn't show the local entry.** Two causes: (a) the feature flag is off; (b) the model-list refresh hasn't fired yet. Force a refresh by signing out and back in, or restart Warp.
- **Selected the local model but request goes to warp.dev anyway.** Check that the LLMId starts with `local:` (model picker subtext should say `Custom local provider`). If somehow a different LLMId is selected, the dispatch router won't fork.
- **Settings TOML edits don't apply.** Warp watches the file but caches reads; restart fixes any persistence issues.
- **`Authorization: Bearer` header missing on outbound.** The key is stored in the OS keychain, not in TOML. Use Settings → AI → Local Provider API Key field, blur out, restart.
- **Tool calls land but Warp does nothing.** Confirm the proto translation succeeded — `typed=true` in the smoke binary's tool-call line. If `typed=false`, the model produced unparseable JSON and you saw the synthetic "tried to call X" message.
