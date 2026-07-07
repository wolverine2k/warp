---
title: "Codebase Indexing and Vector Retrieval Design"
tags: ["architecture", "ai", "codebase-indexing", "embeddings", "vector-retrieval", "design"]
created: 2026-07-07T07:06:03.088Z
updated: 2026-07-07T07:06:03.088Z
sources: []
links: []
category: architecture
confidence: medium
schemaVersion: 1
---

# Codebase Indexing and Vector Retrieval Design

# Codebase Indexing and Vector Retrieval Design

Captured: 2026-07-07.

## Existing Embedding Surface

Warp already contains a built-in full-source-code embedding/indexing subsystem under `crates/ai/src/index/full_source_code_embedding/`. Use this before inventing a parallel codebase vector store inside the app.

Key modules include:

- `changed_files` for change tracking.
- `chunker` for source fragmentation.
- `codebase_index` for index representation.
- `fragment_metadata` for metadata attached to indexed fragments.
- `manager` for lifecycle orchestration.
- `merkle_tree` for sync/cache structure.
- `priority_queue` for indexing order.
- `search_shaping` for query/result shaping.
- `snapshot` for persisted repository snapshots.
- `store_client` for the embedding/vector-store boundary.
- `sync_client` for remote sync behavior.

`crates/ai/src/index/full_source_code_embedding/mod.rs` exposes types such as `EmbeddingConfig`, `RepoMetadata`, `Fragment`, `FragmentLocation`, and `CodebaseContextConfig`. The default embedding config is `Voyage3_5_512`.

## Manager Lifecycle

`crates/ai/src/index/full_source_code_embedding/manager.rs` defines `CodebaseIndexManager`. It coordinates repository metadata, snapshots, changed-file detection, gitignore handling, filesystem watching, debouncing, and persisted snapshot state. The design uses a debounce interval around 10 seconds and a snapshot persistence interval around 10 minutes.

`app/src/lib.rs` wires the manager during app initialization after settings, auth/server API state, persistence, and related AI models are available. `LaunchMode::supports_indexing()` determines whether indexing is enabled for the launch mode. The manager is configured using persisted workspace metadata, server API store clients, user/feature gating, and project context/global rule indexing.

## StoreClient Boundary

`crates/ai/src/index/full_source_code_embedding/store_client.rs` defines `StoreClient`, the key boundary for vector and embedding operations. Its responsibilities include:

- Generate embeddings.
- Update intermediate Merkle nodes.
- Populate Merkle tree cache.
- Sync the Merkle tree.
- Rerank fragments.
- Retrieve relevant fragments.
- Return codebase context configuration.

Future retrieval or bug-fix work should inspect `StoreClient` implementations and `CodebaseIndexManager` call sites before adding new storage or query abstractions.

## Retrieval and Context Design Notes

Fragments contain content, hashes, and fragment locations. Repository metadata and Merkle sync let the system avoid unnecessary recomputation. Search shaping/reranking happens after candidate retrieval and should be kept separate from low-level file watching and chunking.

For agent features, prefer feeding context through the existing codebase context and project context surfaces instead of directly reading arbitrary files in request handlers.

## Invariants and Risks

- Do not create a second codebase vector store inside the Warp app unless the existing `StoreClient` boundary is insufficient and the design tradeoff is documented.
- Keep filesystem watching, snapshot persistence, and embedding generation loosely coupled through `CodebaseIndexManager`.
- Treat fragment hashes and Merkle nodes as cache/sync integrity data; avoid ad hoc mutation.
- Keep query shaping/reranking logic separate from provider adapter protocol logic.
- Respect feature/user gating for codebase indexing, especially for non-GUI launch modes and remote daemon/proxy behavior.
- Avoid logging large source fragments or private code contents in telemetry/errors.
