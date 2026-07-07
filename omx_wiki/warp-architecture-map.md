---
title: "Warp Architecture Map"
tags: ["architecture", "warpui", "workspace", "terminal", "settings", "source-map"]
created: 2026-07-07T07:04:51.025Z
updated: 2026-07-07T07:04:51.025Z
sources: []
links: []
category: architecture
confidence: medium
schemaVersion: 1
---

# Warp Architecture Map

# Warp Architecture Map

Captured: 2026-07-07.

## Repository Shape

This repository is a Rust Cargo workspace. The main application crate is `app/`; reusable crates live under `crates/`. Important product surfaces include `app/src/ai`, `app/src/terminal`, `app/src/workspace`, `app/src/pane_group`, `app/src/settings`, `app/src/settings_view`, `app/src/drive`, `app/src/auth`, and `app/src/persistence`.

Workspace defaults and members are declared in `Cargo.toml`. `app/Cargo.toml` defines the main package `warp` and multiple binaries including `warp-oss`, `warp`, `integration`, `stable`, `dev`, `preview`, and `generate_settings_schema`.

## App Initialization

`app/src/lib.rs` is the central startup surface. `LaunchMode` controls GUI, command-line, test, remote server daemon/proxy, and TUI modes. `LaunchMode::supports_indexing()` gates codebase indexing support, with GUI/test and Agent command-line run paths supporting indexing.

`initialize_app` wires secure storage, settings, auth/server APIs, persistence, cloud resources, `ApiKeyManager`, `AgentProviderSecrets`, AI settings, `LLMPreferences`, codebase index manager, `ProjectContextModel`, global rule indexing, and persisted workspaces. `AgentProviderSecrets` must be registered before `LLMPreferences`, and BYOP legacy migration runs after AI settings and provider secrets exist.

## UI Framework

`crates/warpui_core/src/lib.rs` exports the core UI abstractions: elements, events, presenters, app/context types, colors, geometry, zoom, and layout/rendering primitives. `crates/warpui/src/lib.rs` re-exports `warpui_core` and adds browser, fonts, platform, rendering, windowing, and macros.

WarpUI uses an entity-handle model: a global `App` owns views/models, views hold handles to other views/models, and `AppContext`/view contexts provide scoped access during rendering and events. Mouse input requires `MouseStateHandle` to be created once during construction and reused or cloned; creating inline default handles during render breaks interactions.

## Terminal, Workspace, and Panes

`app/src/terminal/mod.rs` owns terminal modules for blocks, local/remote TTYs, shared sessions, shell integration, terminal model/view/settings, and terminal initialization. `TerminalModel` is a high-risk lock surface: avoid nested `model.lock()` calls, keep lock scopes short, and prefer passing already-locked references down a call stack.

`app/src/workspace/mod.rs` initializes workspace registry, cross-window tab drag, workspace modals/actions, notebooks, code, sync input, LSP, and settings actions. `app/src/pane_group/mod.rs` coordinates pane types such as terminal panes, code panes, notebook panes, settings panes, and related workspace layout behavior.

## Settings, Persistence, GraphQL

`crates/settings/src/lib.rs` separates public and private preferences. Public settings use user-visible TOML when the `SettingsFile` feature is active and otherwise native storage; private settings always use platform-native storage. `crates/settings/src/schema.rs` provides the inventory-backed settings schema registry used by `generate_settings_schema`.

Persistence uses Diesel/SQLite under `crates/persistence`; migrations are under `crates/persistence/migrations` and schema code is generated in `crates/persistence/src/schema.rs`. GraphQL schema/client generation uses `crates/warp_graphql_schema/api/schema.graphql` and generated client surfaces in `crates/graphql`.

## Feature Flags and Testing

Feature flags live in `crates/warp_core/src/features.rs`. Prefer runtime `FeatureFlag::X.is_enabled()` checks for product behavior unless compile-time `cfg` is necessary. Use exhaustive enum matches where possible.

Testing conventions: use `cargo nextest` for workspace tests, `cargo test --doc` for doc tests, integration tests under `crates/integration`, and Rust unit test modules in separate `*_tests.rs` or `mod_test.rs` files included from the owning module. Before PRs or pushed review branches, run `./script/format` and the repo clippy command from `AGENTS.md`/presubmit guidance.
