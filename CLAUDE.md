# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

[`WARP.md`](WARP.md) is the canonical engineering guide for this repo — read it for full coding-style, testing, feature-flag, and platform-specific guidance. This file calls out the parts most likely to bite Claude and the workflows specific to this codebase.

## Commands

Build / run:
- `./script/bootstrap` — platform-specific setup (run once per machine)
- `cargo run` — build and run the Warp client locally
- `cargo run --features with_local_server` — run against a local `warp-server` (override with `SERVER_ROOT_URL` / `WS_SERVER_URL`)
- `cargo bundle --bin warp` — produce a bundled app

Tests:
- `cargo nextest run --no-fail-fast --workspace --exclude command-signatures-v2` — full workspace tests
- `cargo nextest run -p <crate>` — single crate (e.g. `-p warp_completer --features v2`)
- `cargo test --doc` — doc tests
- Integration tests live in `crates/integration/` and use the custom Builder/TestStep framework — when touching user-facing flows, add coverage there (see the `warp-integration-test` skill under `.agents/skills/`)

Presubmit (must pass before pushing a PR):
- `./script/presubmit` — runs fmt + clippy + tests in the same configuration CI uses
- `cargo fmt`
- `cargo clippy --workspace --all-targets --all-features --tests -- -D warnings`
- `./script/run-clang-format.py -r --extensions 'c,h,cpp,m' ./crates/warpui/src/ ./app/src/` — for C/C++/Obj-C edits
- `find . -name "*.wgsl" -exec wgslfmt --check {} +` — for WGSL shader edits

## Architecture

Cargo workspace with ~65 crates under `crates/` plus the main binary in `app/`.

- `app/` — main Warp binary. Houses terminal emulation, AI/Agent Mode, Drive (cloud sync), auth, settings, workspace/session management. `app/src/persistence/schema.rs` is the Diesel/SQLite schema; migrations live alongside.
- `crates/warpui/` and `crates/warpui_core/` — custom Entity-Component-Handle UI framework (the *only* MIT-licensed code; everything else is AGPL-3.0). Global `App` owns entities; views hold `ViewHandle<T>` references; `AppContext` provides temporary access during render/events. Elements describe layout (Flutter-inspired).
- `crates/warp_core/` — core utilities, platform abstractions, and `FeatureFlag` plumbing (`crates/warp_core/src/features.rs`).
- `crates/warp_features/` — feature-flag definitions consumed by client code.
- `crates/integration/` — integration test framework (excluded from default workspace builds; only used for tests).
- `crates/graphql/` — generated client + schema from `graphql/api/schema.graphql`.
- `crates/editor/`, `crates/ipc/`, `crates/lsp/`, `crates/persistence/`, `crates/remote_server/`, etc. — focused libraries used by `app/`.
- `command-signatures-v2/` — separately versioned, excluded from the standard `cargo nextest` invocation above.

Cross-platform: native macOS / Windows / Linux plus a WASM target (see `script/wasm`, `crates/serve-wasm`, `crates/managed_secrets_wasm`).

## Repo-specific landmines

These are easy mistakes that aren't obvious from the code:

- **Terminal model locking.** `TerminalModel::lock()` deadlocks if any caller higher in the stack already holds the lock — the symptom is a frozen UI (macOS beach ball). Before adding a new `model.lock()`, walk up the call stack to confirm no caller already holds it. Prefer threading the locked reference down rather than re-acquiring.
- **`MouseStateHandle` lifetime.** Create it once during construction and clone/reference it everywhere. Calling `MouseStateHandle::default()` inline during render silently breaks all mouse interactions on that view.
- **No `_` wildcards in matches.** This codebase deliberately uses exhaustive matching so adding a new enum variant produces a compile error at every match site. Don't introduce `_ => …` arms unless there's a specific reason.
- **Feature flags over `#[cfg(...)]`.** Gate new behavior with `FeatureFlag::YourFlag.is_enabled()` so it can be toggled without recompilation. Reserve `#[cfg]` for code that genuinely cannot compile without it (platform-specific, missing deps). New flags go in `crates/warp_core/src/features.rs`; rollout lists are `DOGFOOD_FLAGS` / `PREVIEW_FLAGS` / `RELEASE_FLAGS`. The `add-feature-flag`, `promote-feature`, and `remove-feature-flag` skills automate the wiring.
- **Inline format args.** Clippy's `uninlined_format_args` is enforced — write `eprintln!("{message}")`, not `eprintln!("{}", message)`.
- **Unused params get deleted, not `_`-prefixed.** Update the signature and all call sites.
- **Don't churn unrelated comments.** Only modify a comment if the logic it describes changed.
- **Unit-test layout.** Place tests in a sibling `${filename}_tests.rs` (or `mod_test.rs`) and re-include via `#[cfg(test)] #[path = "filename_tests.rs"] mod tests;` at the bottom of the module — not inline `#[cfg(test)] mod tests { … }` blocks.

## Contribution flow

This repo runs an unusual spec-first contribution model driven by Oz (the agent at `oz.warp.dev`). Highlights worth knowing before you propose changes:

- **Issues gate everything.** Discussion happens on the issue, not a speculative PR.
- **Feature requests need a spec PR first.** Once an issue is `ready-to-spec`, add `specs/GH<issue-number>/product.md` (testable behavior invariants) and `specs/GH<issue-number>/tech.md` (implementation plan grounded in current code). Examples: `specs/GH408/`, `specs/GH1063/`, `specs/GH1066/`. The `write-product-spec` and `write-tech-spec` skills scaffold these. Implementation typically continues on the same PR after spec approval.
- **Bug fixes skip the spec step** — all triaged bugs are implicitly `ready-to-implement`.
- **PRs use the template at `.github/pull_request_template.md`** and should include a changelog entry: `CHANGELOG-NEW-FEATURE:`, `CHANGELOG-IMPROVEMENT:`, or `CHANGELOG-BUG-FIX:` (omit for docs/refactor-only changes).
- **Branch naming:** prefix with your handle, e.g. `alice/fix-parser`.
- **Reviewers are auto-assigned.** Don't request human reviewers manually — Oz reviews first, then routes to a Warp SME. Comment `/oz-review` on the PR (max 3×) after pushing fixes.

## Repo skills

Useful agent skills under `.agents/skills/` that map to common tasks here:

- `rust-unit-tests`, `warp-integration-test` — writing/running tests
- `warp-ui-guidelines` — WarpUI patterns to consult before any UI change
- `add-feature-flag`, `promote-feature`, `remove-feature-flag` — feature-flag lifecycle
- `add-telemetry` — wiring telemetry events
- `write-product-spec`, `write-tech-spec`, `implement-specs`, `spec-driven-implementation` — spec flow
- `create-pr`, `review-pr`, `review-pr-local`, `diagnose-ci-failures`, `fix-errors` — PR mechanics
- `resolve-merge-conflicts`, `triage-issue-local`, `dedupe-issue-local` — repo housekeeping

When tackling work that matches one of these (touching UI, adding/removing flags, writing tests, opening a PR, etc.), invoke the corresponding skill rather than improvising.
