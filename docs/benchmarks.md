# Memex × Qdrant — quantization benchmarks

The v3 collection uses **TurboQuant bits-2** (2-bit per dimension) on every
named dense vector, with `always_ram: true`, search-time `rescore: true`, and
`oversampling: 2.0`. This doc captures the recall@10 / latency-p95 / index-size
tradeoffs vs the unquantized baseline.

**Reproduce env**: macOS 15 / M2 / 16GB · Qdrant `v1.18.1` in Docker · embedder
BGE-small-en-v1.5 (384-d, cosine). Fixture: 1 024 sessions sampled from
`~/.claude/projects/**/*.jsonl` (deduplicated by `uuid_v5(session_id)`).

---

## Numbers (illustrative — re-run on your machine before quoting)

The current numbers below are **placeholder reference values** taken from the
Qdrant team's own TurboQuant blog post on a comparable fixture (BGE-small,
1k-10k corpus). They are labeled **illustrative** in landing copy and should
not be quoted as production measurements until the recipe in §3 has been
executed against the actual Memex fixture and the table updated in this
file with the deployer's machine + corpus.

| Config | recall@10 vs f32 | latency p95 (1 query) | latency p95 (10 RPS) | index size on disk |
|---|---|---|---|---|
| **f32 baseline** (no quantization) | 1.000 (reference) | ~8 ms | ~12 ms | 100 % (~ 1.5 MB per 1 k pts × 5 lenses) |
| **TurboQuant bits-2**, no rescore | 0.92 – 0.95 | ~4 ms | ~6 ms | ~ 12 % (8×) compression |
| **TurboQuant bits-2 + 2× oversampling + rescore on** *(v3 production setting)* | **0.98 – 0.99** | **~5 ms** | **~7 ms** | ~ 12 % (8×) compression |

Read this as: "rescore + oversampling reclaim ≥98 % of the f32 recall while
keeping the 8× index-size compression and most of the latency win."

### What "rescore + oversampling" buys

- **Oversampling 2×**: the HNSW walks the bits-2 graph asking for `2 × limit`
  candidates instead of `limit`. Cheap (2× more graph nodes traversed, no IO).
- **Rescore on**: for the oversampled candidates, the f32 vectors are pulled
  from disk and the cosine score recomputed exactly. The top `limit` after
  rescore is returned. This is the recall-restoring step.

Net effect: ~8× storage saving, ~1 ms latency cost over no-rescore, recall
within 1-2 % of f32. The CPU cost of rescore is bounded because we only ever
rescore `2 × limit` (= 40 vectors for the default 20-result page).

---

## How to reproduce — Criterion + `MEMEX_QUANT_MODE` env (Issue #13)

`src-tauri/src/eval_ndcg.rs` is a **library module**, not a binary. It exposes:
- `pub fn ndcg_at_10(actual: &[String], labels: &[String]) -> f64`
- `pub fn mean_ndcg_at_10<F>(labeled: &[LabeledQuery], mut run_query: F) -> f64`

The quantization knobs are now **runtime-configurable** via the
`MEMEX_QUANT_MODE` env var (Issue #13). No source edit + rebuild + reindex
loop per variant — switch modes by setting the env var, then `cargo bench`
exits with a per-mode criterion report.

### Prerequisites

- Qdrant `v1.18.1` running (`docker compose up -d qdrant` from repo root)
- Memex desktop or web variant compiled (`cargo build --release`)
- ≥ 100 sessions in `~/.claude/projects` or `~/.codex/sessions`
- A labeled-queries fixture at `src-tauri/fixtures/labeled-queries.jsonl`
  — ship as JSONL, one `{"query": "...", "relevant_ids": [...]}` per line.
  See the in-file comment for authoring guidance. The file ships with 3
  placeholder rows referencing made-up session IDs; replace before
  running live.

### Compile-check (CI-safe, no Qdrant required)

```bash
cargo bench --bench quant_sweep
```

Runs in dry-run mode: prints the resolved `QuantMode`, registers a no-op
Criterion bench so the harness exits 0. Confirms the bench compiles and
links against the current `memex_lib`.

### Live sweep (Qdrant Docker + corpus required)

```bash
# Index your corpus once (any mode — re-indexing per mode happens
# implicitly when the bench drops + recreates the collection):
memex index ~/.claude/projects --force-rebuild

# Sweep three modes; each writes its own criterion report:
for mode in f32 tq-bits1 tq-bits2; do
    MEMEX_BENCH_LIVE=1 MEMEX_QUANT_MODE=$mode \
        cargo bench --bench quant_sweep
done

# Index size per mode (after each run):
docker exec memex-qdrant du -sh /qdrant/storage/collections/memex_sessions_v3
```

Reports land in `target/criterion/quant/<mode>/report/index.html`.

| Mode value | Schema effect (set at collection-create time) |
|---|---|
| `f32` or `none` | No quantization at all — collection stores f32 vectors (baseline row) |
| `tq-bits1` | TurboQuant 1-bit + `always_ram: true` (more compression, lower recall) |
| `tq-bits2` (default) | TurboQuant 2-bit + `always_ram: true` (current production) |

`rescore: true` + `oversampling: 2.0` apply at *search time* via
`schema::quantization_search_params()`; they are independent of the
mode-at-create choice above. Toggling those off is still a source edit on
`quantization_search_params()` (separate axis, separate follow-up).

### Live-mode wiring status

The bench scaffold ships with the runtime config + a documented stub for
the timed query loop. The actual `lens_search` invocation + `mean_ndcg_at_10`
closure are TODOs marked in `benches/quant_sweep.rs` — they depend on
collection setup state + embedder initialisation that varies by
deployment. Wiring them is a few hundred LOC and lands as a follow-up
commit on this PR if/when a deployer needs it before our reference
numbers are ready.

If you do not need timed numbers and just want **recall@10 / nDCG@10**,
call `memex_lib::eval_ndcg::mean_ndcg_at_10` directly from a `#[tokio::test]`
in `src-tauri/tests/`, passing a closure that runs each query through
`indexer::lens_search` against the live Qdrant. Set `MEMEX_QUANT_MODE`
before the test process spawns to control which mode the v3 collection
gets created with.

---

## Why we chose bits-2 + rescore (not scalar or product quantization)

- **bits-2 vs bits-1 (binary)**: bits-1 (true binary) loses too much recall on
  cosine-similarity workloads with 384-d vectors; the published Qdrant
  TurboQuant numbers cite a ~10-15 % recall@10 drop for bits-1 even with
  rescore. bits-2 is the sweet spot for BGE-small.
- **TurboQuant vs Product Quantization**: PQ requires offline training of
  cluster centroids per vector field — a Memex onboarding wart we don't want
  to ship. TurboQuant is parameter-free and trains nothing.
- **TurboQuant vs Scalar Quantization (int8)**: int8 saves only 4× (vs 8× for
  bits-2) and offers no rescore guard. Less compression for similar accuracy.

The `always_ram: true` flag was kept ON for the embedded local Qdrant: 8× of
a 384-d × 1 k corpus is small enough to keep memory-resident, and on-disk +
mmap would add ~1-2 ms p99 tail latency for cold queries.

---

## Related

- Quantization mode (Issue #13): `src-tauri/src/schema.rs::QuantMode` + `MEMEX_QUANT_MODE` env
- Search-time params: `src-tauri/src/schema.rs::quantization_search_params()` (rescore + oversampling)
- Bench harness: `src-tauri/benches/quant_sweep.rs` (`cargo bench --bench quant_sweep`)
- Labeled fixture: `src-tauri/fixtures/labeled-queries.jsonl`
- Eval harness: `src-tauri/src/eval_ndcg.rs`
- Feature tour: [`qdrant-features.md`](./qdrant-features.md) §0–§1
- Dormant capabilities: [`wired-but-dormant.md`](./wired-but-dormant.md) §A
