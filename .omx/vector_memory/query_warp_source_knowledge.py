#!/usr/bin/env python3
from __future__ import annotations

import argparse
import collections
import hashlib
import math
import re
import sqlite3
import struct
from pathlib import Path

DIM = 2048
TOKEN_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]{2,}")
HEXISH_RE = re.compile(r"^(?:x)?[0-9a-f]{2,}$")


def tokens(text: str) -> list[str]:
    out = []
    for token in TOKEN_RE.findall(text):
        t = token.lower().strip("_")
        if len(t) < 3 or HEXISH_RE.match(t):
            continue
        out.append(t)
        for part in re.split(r"[_:/.-]+", t):
            if len(part) >= 3 and not HEXISH_RE.match(part):
                out.append(part)
    return out


def pack_embedding(token_counts: collections.Counter[str], idf: dict[str, float]) -> bytes:
    vec = [0.0] * DIM
    max_tf = max(token_counts.values(), default=1)
    for token, count in token_counts.items():
        digest = hashlib.blake2b(token.encode("utf-8"), digest_size=8).digest()
        value = int.from_bytes(digest, "little")
        idx = value % DIM
        sign = 1.0 if ((value >> 13) & 1) else -1.0
        tf = 0.5 + 0.5 * (count / max_tf)
        vec[idx] += sign * tf * idf.get(token, 1.0)
    norm = math.sqrt(sum(v * v for v in vec))
    if norm:
        vec = [v / norm for v in vec]
    return struct.pack(f"<{DIM}f", *vec)


def unpack_embedding(blob: bytes) -> tuple[float, ...]:
    return struct.unpack(f"<{DIM}f", blob)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Query the local Warp source knowledge vector store."
    )
    parser.add_argument("query", help="Natural-language or code-symbol query.")
    parser.add_argument("--limit", type=int, default=8)
    parser.add_argument(
        "--db",
        default=Path(__file__).with_name("warp_source_knowledge.sqlite"),
        type=Path,
    )
    args = parser.parse_args()

    conn = sqlite3.connect(args.db)
    idf = dict(conn.execute("SELECT token, idf FROM token_idf"))
    qv = unpack_embedding(pack_embedding(collections.Counter(tokens(args.query)), idf))

    results = []
    for path, kind, chunk_index, content, blob in conn.execute(
        "SELECT path, kind, chunk_index, content, vector FROM chunks"
    ):
        vec = unpack_embedding(blob)
        score = sum(a * b for a, b in zip(qv, vec))
        snippet = " ".join(content.split())[:220]
        results.append((score, path, kind, chunk_index, snippet))

    for score, path, kind, chunk_index, snippet in sorted(results, reverse=True)[: args.limit]:
        print(f"{score:.4f}\t{kind}\t{path}#{chunk_index}\t{snippet}")


if __name__ == "__main__":
    main()
