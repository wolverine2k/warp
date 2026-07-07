---
title: "Local Vector Knowledge Store"
tags: ["vector-database", "source-map", "memory", "query", "design"]
created: 2026-07-07T07:11:33.918Z
updated: 2026-07-07T07:11:33.918Z
sources: []
links: []
category: reference
confidence: medium
schemaVersion: 1
---

# Local Vector Knowledge Store

# Local Vector Knowledge Store

Captured: 2026-07-07.

## Purpose

A local SQLite-backed vector store was created to persist source-code understanding and design notes for later bug fixing and feature implementation.

Database path:

`.omx/vector_memory/warp_source_knowledge.sqlite`

Query helper:

`python3 .omx/vector_memory/query_warp_source_knowledge.py "BYOP AgentProviderSecrets local_provider" --limit 8`

`.omx/` is ignored by this checkout, so the vector database persists locally across launches without adding a large binary artifact to normal source control.

## Stored Content

The vector DB stores:

- Source chunks from text/code files outside `target/`, `.git/`, dependency/vendor/build-output folders, and low-signal generated resource bundles.
- Wiki/design chunks from `omx_wiki/*.md`, excluding wiki maintenance files such as index/log in the final rebuild.
- Synthesis chunks for architecture, Local-Warp BYOP design, codebase vector retrieval design, release tagging design, and the subagent permission rule.

## Schema

`metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL)`

`chunks(id INTEGER PRIMARY KEY, path TEXT, title TEXT, kind TEXT, chunk_index INTEGER, content TEXT, vector BLOB)`

`token_idf(token TEXT PRIMARY KEY, idf REAL NOT NULL)`

Vectors are deterministic TF-IDF weighted signed-hashing embeddings over tokenized text, path, title, and kind hints. The embedding dimension is 2048 float32 values stored as a BLOB.

## Design Included

The design itself is stored in both wiki pages and vector DB chunks. Important design pages include:

- `Warp Architecture Map`
- `Local-Warp BYOP Provider Design`
- `Codebase Indexing and Vector Retrieval Design`
- `Release Tagging and Resource Packaging Workflow`
- `Warp Agent Rules and Engineering Conventions`
- `Local Vector Knowledge Store`

## Query Notes

Use the query helper for quick retrieval. Good queries combine feature names, file/symbol names, and intent, for example:

- `BYOP AgentProviderSecrets provider adapter run_chat_turn`
- `CodebaseIndexManager StoreClient embeddings vector retrieval`
- `release tag validate_release_tag oss prepare bundled resources`
- `standing permission spawn Codex native subagents`

This is a local agent-memory vector store, not a replacement for Warp's in-product `full_source_code_embedding` subsystem.
