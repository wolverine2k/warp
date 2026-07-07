---
title: "Warp Agent Rules and Engineering Conventions"
tags: ["conventions", "agents", "subagents", "testing", "safety", "rules"]
created: 2026-07-07T07:06:31.972Z
updated: 2026-07-07T07:06:31.972Z
sources: []
links: []
category: convention
confidence: medium
schemaVersion: 1
---

# Warp Agent Rules and Engineering Conventions

# Warp Agent Rules and Engineering Conventions

Captured: 2026-07-07.

## Agent Orchestration

The repository owner grants standing permission to spawn Codex native subagents for independent, bounded repository work when doing so improves throughput, coverage, or verification quality. Do not ask for separate subagent permission. Continue to use normal engineering judgment for destructive actions, external side effects, shared-file conflicts, and scope expansion.

This rule is also persisted in `AGENTS.md` under `Agent Orchestration` and in OMX project memory.

## Commit Protocol

Commit messages should follow the Lore protocol from the top-level instructions: intent line first, narrative body when useful, and git-native trailers such as `Constraint:`, `Rejected:`, `Confidence:`, `Scope-risk:`, `Directive:`, `Tested:`, and `Not-tested:`.

## Release Tags

Use only tags matching `script/validate_release_tag`: `v<major>.<YYYY>.<MM>.<DD>.<HH>.<mm>.<oss|stable|preview|dev>_<NN>`. Do not use `github-*` or other tag formats for package releases.

## Code Change Discipline

- Keep diffs small, reviewable, and reversible.
- Prefer existing repo patterns and utilities over new abstractions.
- No new dependencies without explicit request.
- Do not revert user changes or unrelated dirty worktree changes.
- Use `apply_patch` for manual source edits.
- Preserve comments unless the described logic changes.

## Rust/Repo Style

- Avoid unnecessary type annotations, especially closure params.
- Prefer imports over long Rust path qualifiers, except for small cfg-guarded cases.
- Context parameters should be named `ctx` and usually come last.
- Remove unused parameters completely rather than prefixing them with `_`.
- Prefer inline format args such as `format!("{message}")`.
- Do not pass `Itertools::format` directly to logging macros; materialize a reusable string first.
- Avoid wildcard `_` match arms when exhaustive matching is practical.

## High-Risk Surfaces

- `TerminalModel::lock()` can deadlock if nested; keep locks short and pass locked references when possible.
- `MouseStateHandle` must be created once during construction and reused/cloned; inline render-time defaults break mouse interaction.
- BYOP secrets must stay in secure storage or managed secret references.
- Codebase vector retrieval should use the existing `full_source_code_embedding` / `CodebaseIndexManager` / `StoreClient` path.

## Verification

Before PRs or pushed review branches, run `./script/format` and the repo clippy command specified in `AGENTS.md`/presubmit guidance. For standard Rust verification, prefer `cargo nextest` where appropriate and targeted `cargo test` for narrow packages.
