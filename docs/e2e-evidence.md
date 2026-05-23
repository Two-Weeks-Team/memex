# E2E verification evidence

Concise, reproducible proof that Memex builds, tests, starts Qdrant, indexes a
corpus, and answers across all Qdrant-backed surfaces. Every output below comes
from the **committed synthetic** `examples/sample-corpus/` (no private data), so
it is publishable as-is.

- **Run date:** 2026-05-23
- **Host:** macOS (Darwin arm64) · `cargo 1.93.0` · `docker 29.3.1` + compose v2 · `node v24.15.0`
- **Qdrant:** `qdrant/qdrant:v1.18.0` via `docker-compose.yml`

Private-corpus E2E (the author's real `~/.claude/projects`) is validated
separately by `scripts/demo/smoke-test.sh`; its raw artifacts are **gitignored**
because they contain real session UUIDs, home paths, and project names (see
[tests/e2e/README.md](../tests/e2e/README.md)). This page deliberately uses the
synthetic corpus so the proof can live in git without leaking anything.

---

## 1. Tests

```bash
cargo test --manifest-path src-tauri/Cargo.toml --locked -- --test-threads=1
```

**Result: 228 passed · 0 failed · 4 ignored** (exit 0). Per binary:

| Suite | Passed | Ignored |
|---|---|---|
| `unittests src/lib.rs` | 175 | 3 |
| `codex_parser_integration` | 11 | |
| `lens_integration` | 6 | 1 |
| `parser` | 14 | |
| `retrieval_integration` | 6 | |
| `schema_integration` | 12 | |
| `sec_integration` | 2 | |
| `snapshot_integration` | 2 | |
| **Total** | **228** | **4** |

Integration tests run for real here because Qdrant was up. CI instead sets
`MEMEX_SKIP_QDRANT_TESTS=1` (the in-repo "CI fallback" honored by the
lens/retrieval/schema suites) so those skip and CI stays green without a Qdrant
service — the Qdrant-backed coverage is this document.

> **Known flake (not a regression):** the default *parallel* `cargo test` run
> produced 1 failure in `schema_integration::it_quantization_present_in_collection_info`
> — Qdrant returned `Collection ... already exists!`. The cause is a
> nanosecond-window collision in the test helper `unique_collection()` (whose
> own comment notes clashes are "rare but possible"). The test **passes in
> isolation** and the **full suite passes single-threaded** (above). It is a
> test-isolation race, independent of this PR (no Rust source changed). CI is
> unaffected because integration tests self-skip without Qdrant.

## 2. Build

```bash
cargo build --release --manifest-path src-tauri/Cargo.toml --locked
#   Finished `release` profile [optimized] target(s) in 57.72s   (exit 0)

file src-tauri/target/release/memex
#   Mach-O 64-bit executable arm64
```

## 3. Qdrant startup

```bash
bash scripts/start-qdrant.sh
#   Container memex-qdrant Started
#   Waiting for Qdrant to become ready at http://localhost:6333/readyz . ready
#   ✓ Qdrant is up.  gRPC (Memex uses): http://localhost:6334

curl -fsS http://localhost:6333/readyz   # → all shards are ready
curl -fsS http://localhost:6333/         # → {"title":"qdrant - vector search engine","version":"1.18.0"}
```

gRPC listener confirmed up on `:6334` (the port `MEMEX_QDRANT_URL` points at).

## 4. Sample corpus import

```bash
./src-tauri/target/release/memex scan --path examples/sample-corpus --index
#   parsed 12 session(s) (shown: 12), 33 total tool calls
#   indexed 12/12 session(s) into 'memex_sessions_v3' (0 duplicate sessionId(s) skipped, 0 error(s))

curl -fsS http://localhost:6333/collections/memex_sessions_v3 \
  | jq '{status:.result.status, points:.result.points_count, vectors:(.result.config.params.vectors|keys)}'
#   { "status": "green", "points": 12,
#     "vectors": ["code","content","content_late","error","path","tool"] }
```

Six named vectors per session — the foundation for the lens surface.

## 5. Qdrant-backed surfaces (synthetic corpus, real output)

```text
$ memex search "rate limiter redis" --limit 3
1.0252  acme-api     321dd4d0…   (fix/rate-limit — exact match)
0.8899  ml-pipeline  e1072ed7…
0.7427  acme-api     b2e0216f…

$ memex lens "build error" --error 2.0 --content 1.0 --limit 3   # error-weighted multi-vector
5.6448  infra        9ad3e88f…   (fix/ci-cache  — cargo linker error)
5.4440  acme-web     e7bea3eb…   (fix/build-fail — module not found)
3.2947  ml-pipeline  ed807207…

$ memex recall "cargo build linker error" --limit 3              # proactive recall
0.9293  infra        9ad3e88f…   ← the session that fixed exactly this
0.6564  acme-web     e7bea3eb…
0.6206  acme-api     321dd4d0…

$ memex mix --pos 046df7e8… --neg e1072ed7… --limit 3            # Discovery API
1.6834  acme-web     883eb8c3…   (like jwt-auth, unlike training-nan)
1.6700  infra        eedd4028…
-0.3315 acme-web     e7bea3eb…   (correctly pushed away)

$ memex topology --sample 12 --per-point 4 --out /tmp/topo.json  # Distance Matrix → MST
topology: 12 node(s), 11 MST edge(s), 1 gap(s)

$ memex predict 321dd4d0… --neighbors 5     # requires corpus under ~/.claude/projects (sandbox)
1  Bash  python train.py --steps 500   from ml-pipeline (turn #6)
```

`predict`/`replay` re-read source `.jsonl` through the `sec.rs` path sandbox
(roots: `~/.claude/projects`, `~/.codex/sessions`); see
[examples/sample-corpus/README.md](../examples/sample-corpus/README.md) for the
tested copy-into-root workaround used to produce the `predict` line above.

## 6. Distribution binary signing (honesty check)

```bash
codesign -dv src-tauri/target/release/bundle/macos/Memex.app
#   flags=0x20002(adhoc,linker-signed)  Signature=adhoc  TeamIdentifier=not set
```

Ad-hoc signed, **not** notarized — the reason for the Gatekeeper steps in
[docs/INSTALL.md](INSTALL.md).
