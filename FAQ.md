# Frequently Asked Questions

This FAQ covers the questions we hear most often about Local-Warp, contributing to the Warp client fork, working with agents in this repository, and how this repo fits into Warp/OpenWarp. For the full contribution flow, see [CONTRIBUTING.md](CONTRIBUTING.md). For engineering details — build setup, code style, testing — see [AGENTS.md](AGENTS.md).

## Local-Warp, BYOK, and BYOP

### What is Local-Warp?

Local-Warp is a fork of Warp/OpenWarp focused on provider autonomy. It keeps the terminal-native agent workflow and adds **Bring Your Own Key (BYOK)** and **Bring Your Own Provider (BYOP)** so users can route agent traffic through their own credentials and endpoints.

### What does BYOK mean in Local-Warp?

**Bring Your Own Key** means provider credentials are supplied by the user instead of by a shared managed provider account. Local-Warp stores provider API keys in the OS keychain through `AgentProviderSecrets` and resolves them only when dispatching a request.

### What does BYOP mean in Local-Warp?

**Bring Your Own Provider** means the user can configure one or more provider endpoints under `agents.warp_agent.providers`, including local Ollama/LM Studio/vLLM-style servers, OpenAI-compatible gateways, Anthropic, Gemini, DeepSeek, and other supported API types. Conversations and orchestration runs can target a concrete `byop:<provider_id>:<model_id>` model.

### Are provider keys sent to Warp?

Interactive BYOK/BYOP dispatch uses the configured provider endpoint and the user's key for that provider. Remote-worker BYOP support forwards provider metadata and managed-secret names so workers can resolve credentials without embedding raw keys in request payloads. Raw API keys should never be committed, logged, pasted into issues, or included in screenshots.

## Contributing

### How do I contribute?

Start with a GitHub issue. Bug reports can go straight to a code PR once they are triaged as actionable; feature requests go through a short spec PR before any code is written. The full flow — readiness labels, spec PRs, code PRs, review — is documented in [CONTRIBUTING.md](CONTRIBUTING.md).

### How do I file a good bug report or feature request?

Use the [issue templates](https://github.com/warpdotdev/warp/issues/new/choose). For bugs, include reproduction steps, expected vs. actual behavior, your Warp version (`Settings → About`), and OS. For features, describe the user-facing problem before proposing an implementation.

If you're already running Warp, the `/feedback` command files an issue with logs and environment details attached automatically.

### What do the readiness labels mean?

- **`ready-to-spec`** — the problem is understood, the design is open. Next step is a spec PR.
- **`ready-to-implement`** — the issue is ready for a code PR. For bugs, this means the report is sufficiently reproducible or actionable.
- **`needs-mocks`** — design mocks are required before implementation can start.

Anyone can pick up a labeled issue. Mention **@oss-maintainers** on an issue if it needs triage or readiness re-evaluation.

### Why do features need a spec PR before code?

Specs make scope, behavior, and architecture reviewable on their own, before someone writes code that may need to be thrown away. Each spec PR adds a `product.md` (desired behavior) and a `tech.md` (implementation plan) under `specs/GH<issue-number>/`. See [Opening a Spec PR](CONTRIBUTING.md#opening-a-spec-pr) for what each document should contain.

### How do I build and run Local-Warp from source?

```bash
./script/bootstrap   # platform-specific setup
./script/run         # build and run Local-Warp
./script/presubmit   # fmt, clippy, and tests
```

macOS, Linux, and Windows are all supported. Platform-specific setup is handled by `./script/bootstrap`. See [AGENTS.md](AGENTS.md) for the full engineering guide.

### Will my PR be reviewed by a human or by an agent?

Both. When you open a PR, Oz is auto-assigned and produces an initial review. Once Oz approves, it automatically requests a follow-up review from a Warp team subject-matter expert. You don't need to assign reviewers manually.

### My PR has been sitting without review — what do I do?

After you push changes that address Oz's feedback, comment `/oz-review` on the PR (up to three times per PR) to request a re-review. If something looks stuck or you've used your re-reviews, mention **@oss-maintainers** to escalate to the team.

### What's the difference between a contributor and a collaborator?

A **contributor** is anyone who contributes to the project — by filing an issue, opening a PR, helping triage, or participating in discussion. Most people who help out are contributors. You don't need permission or a status of any kind; just file an issue or open a PR.

A **collaborator** is a formal GitHub role we grant to contributors with a track record of merged PRs in this repo. Collaborators get expanded permissions: applying and managing issue labels, dispatching Oz directly with `@oz` on any ready issue, and using complimentary Oz credits for work in this repo.

### How do I become a collaborator?

Contributors with several merged PRs may be invited to become collaborators. There's no formal application — keep contributing, and a maintainer will reach out.

## Using an agent on this repo

### Can I use my own coding agent to contribute?

Yes. Use whatever you like — Warp's built-in agent, Claude Code, Codex, Gemini CLI, Cursor, others, or no agent at all. The repo ships agent-readable context (skills under [`.agents/skills/`](.agents/skills/), specs under [`specs/`](specs/), and [`AGENTS.md`](AGENTS.md)) that any harness supporting these formats can pick up.

### Can I use Codex, Claude, Gemini, or other provider credentials in Local-Warp?

Yes, where the provider or harness exposes a supported API path. BYOK/BYOP settings let you add Anthropic, OpenAI-compatible, Gemini, DeepSeek, and Ollama-style providers, and Local-Warp can route supported local child harnesses at selected BYOP endpoints. See [`specs/multi-local-llm/README.md`](specs/multi-local-llm/README.md) for the current support matrix and smoke-test gates.

### How can I get Oz to implement an issue for me?

Mention **@oss-maintainers** on any issue with a readiness label and ask. Approved requests run on **complimentary Oz credits** — you don't need to set up your own Oz account or pay for compute.

Once you're a collaborator, you can mention `@oz` directly on any ready issue to dispatch it without waiting for a maintainer.

### Do I have to pay anything to contribute here?

No. Contributing by hand or with your own agent is free. Oz runs on Warp's credits for approved requests on this repo, and is free for collaborators contributing back to it.

### Are agent-generated PRs held to the same bar as human ones?

Yes. The same Oz + SME review, the same tests, and the same `./script/format` / `cargo clippy` / presubmit checks apply regardless of who (or what) wrote the code. Whether a PR is hand-written or agent-written doesn't change the quality bar — it changes how quickly you can iterate to meet it.

### Will my issues, comments, or code be used to train models?

No. Warp does not use contributions to this repository, or the discussion around them, for model training.

## What's open source and what isn't

### Is Warp fully open source?

The Warp **client** is open source: the app and most crates are licensed under [AGPL v3](LICENSE-AGPL), and the UI framework crates (`warpui_core`, `warpui`) are licensed under [MIT](LICENSE-MIT). The **server**, the **Warp Drive backend**, and **Oz** (our agent orchestration layer) are not in this repository and remain proprietary today.

### What lives in this repo and what doesn't?

**In this repo:** the Warp client app, the WarpUI framework, integration tests, agent skills, and feature specs.

**Not in this repo:** the server, the Drive backend, hosted authentication, and Oz orchestration.

### Can I run Local-Warp without signing in or using Warp's cloud?

Some functionality works fully locally, and BYOK/BYOP is designed to expand the locally-runnable agent surface by routing inference to user-configured providers. Other features, including Drive sync, hosted-model agents, team features, and some Remote-worker paths, still depend on Warp backend services.

### Will the server or Oz ever be open-sourced?

We haven't committed to a date and don't want to overpromise. Opening the client under AGPL is a one-way door, and opening the server would be a similar commitment — we'll be explicit when and if we make it.

## Licensing

### Why did you pick this license — AGPL for the app and MIT for the UI crates?

We wanted two different things from each part of the codebase, so we picked two different licenses.

For the **client app**, we chose [AGPL v3](LICENSE-AGPL) because we wanted modifications to stay open. A permissive license like MIT or Apache 2.0 would let someone fork the client, make changes, and ship a closed-source product back to users — that's a pattern we've seen burn end-user-facing open source projects, and it's not the ecosystem we want to seed. AGPL closes the network-use loophole that GPL leaves open, so a hosted derivative of the client is also covered. The trade-off is that AGPL is stricter than what some companies are comfortable embedding into proprietary products, and we accept that — the client isn't where we expect that kind of reuse.

For the **UI framework crates** (`warpui_core`, `warpui`), we chose [MIT](LICENSE-MIT) because they're general-purpose infrastructure that's useful well outside Warp. We want people building unrelated apps in Rust to be able to pick them up without the friction AGPL introduces. Keeping that layer permissive is good for the framework's reach and good for upstream contributions back to it.

In short: AGPL where we want derivatives to stay open, MIT where we want maximum reuse.

### Can I use Local-Warp at my company under AGPL?

Yes. Using Local-Warp as your terminal or development environment doesn't trigger AGPL's network or distribution obligations. AGPL applies if you modify the client *and* distribute or host that modified version for others.

### Why is there a CLA?

The CLA grants Warp the rights it needs to redistribute contributions under this project's licenses (AGPL and MIT) and to address future licensing and compliance needs. It does not change the license of code contributed to this repo.

### Can someone fork Local-Warp or Warp?

Yes — that's what AGPL is for. Local-Warp itself is a fork focused on BYOK/BYOP. The license prevents fully-proprietary relaunches; open derivatives are welcome.

## Help and security

### Where do I get help?

- The [Warp docs](https://docs.warp.dev/) for using the product.
- [GitHub Issues](https://github.com/warpdotdev/warp/issues) for bug reports and feature requests.
- The [Slack community](https://go.warp.dev/join-preview) for general questions and discussion — contributors chat with each other and the Warp team in [`#oss-contributors`](https://warpcommunity.slack.com/archives/C0B0LM8N4DB).
- Mention **@oss-maintainers** on an issue or PR to escalate to the team.

### How do I report a security vulnerability?

Please don't open a public GitHub issue. See [SECURITY.md](SECURITY.md) — report via [security@warp.dev](mailto:security@warp.dev) or open a private [GitHub Security Advisory](https://github.com/warpdotdev/Warp/security/advisories/new).
