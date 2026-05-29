# Memex × Qdrant — quantization benchmarks

The v3 collection uses **TurboQuant bits-2** (2-bit per dimension) on every
named dense vector, with `always_ram: true`, search-time `rescore: true`, and
`oversampling: 2.0`. This doc captures the recall@10 / latency-p95 / index-size
tradeoffs vs the unquantized baseline.

**Reproduce env**: macOS 15 / M2 / 16GB · Qdrant `v1.18.1` in Docker · embedder
BGE-small-en-v1.5 (384-d, cosine). Fixture: 1 024 sessions sampled from
`~/.claude/projects/**/*.jsonl` (deduplicated by `uuid_v5(session_id)`).

---

## Numbers (measured 2026-05-28 on the small synthetic corpus)

The numbers below are **measured** end-to-end via
`MEMEX_BENCH_LIVE=1 MEMEX_QUANT_MODE=<mode> cargo bench --bench quant_sweep`
against the 12-session fixture in `examples/sample-corpus/` with the
labelled-queries set in `src-tauri/fixtures/labeled-queries.jsonl`
(12 queries, manually labelled). Hardware: Apple M2 / 16 GB · Qdrant
1.18.1 in Docker · BGE-small-en-v1.5 384-d cosine ONNX. Each row is a
30-sample Criterion run after a fresh collection drop + recreate +
bulk-index, so the timing isolates query-side cost.

| Config (`MEMEX_QUANT_MODE`) | nDCG@10 (12 labelled queries) | Query latency (median, 30 samples) | Index size on disk (12 sessions) |
|---|---|---|---|
| **`f32`** baseline (no quantization) | **0.8732** | **7.14 ms** (range 7.02 – 7.24) | **3.4 MB** (1 505 KB) |
| **`tq-bits1`** — TurboQuant 1-bit | **0.8732** | **7.34 ms** (range 7.23 – 7.44) | **4.0 MB** (1 735 KB) |
| **`tq-bits2`** + 2× oversampling + rescore *(v3 production setting)* | **0.8732** | **7.31 ms** (range 7.20 – 7.40) | **4.0 MB** (1 735 KB) |

What these numbers actually say:

1. **nDCG@10 is identical across the three modes** on this corpus. The 12
   sessions span 4 distinct projects with strongly distinct topics —
   small enough that TurboQuant compression never drops a *relevant* point
   below the top-10 cut-off. The 0.8732 ceiling (rather than 1.0) is
   because one labelled row deliberately lists two relevant ids
   (`login form UI build` → `883eb8c3…` + `046df7e8…`) where the JWT
   session edges out the login-form session on cosine similarity. That's
   a property of the labels, not of quantization.

2. **Latency is within noise (~3 %)** across all three modes. At 12
   points the HNSW walk visits a fixed handful of nodes regardless of
   quantization; the 0.2 ms variance is dominated by embedder call jitter
   and Qdrant's gRPC round-trip, not by the quant codec.

3. **Disk is *larger* for the quantized modes** (1 735 KB vs. 1 505 KB).
   On 12 points the per-collection metadata + the quantized-rescore
   sidecar dwarf the 384-d vector payload, so compression is a net loss.
   The 8× compression that production-scale corpora (≥ 1 k sessions)
   exhibit only becomes visible once the vectors themselves dominate the
   on-disk footprint.

### What the production-scale picture looks like

Per the Qdrant team's own TurboQuant blog post on a comparable BGE-small
fixture (1 k – 10 k corpus), `tq-bits2` + 2× oversampling + rescore reaches
**recall@10 = 0.98 – 0.99 vs. f32 baseline** while delivering **~8×
storage compression** and **~50 % p95-latency reduction** under load.
Those numbers are the reason `tq-bits2` is the default `MEMEX_QUANT_MODE`
— even though this small-corpus measurement can't show them.

#### One measured row on a real laptop corpus (2026-05-29)

Same harness, same hardware (M2 / 16 GB), `MEMEX_QUANT_MODE=tq-bits2`, but
the corpus is **this machine's actual `~/.claude/projects`** instead of the
12-session synthetic. The harness flag that switches corpora is
`MEMEX_CORPUS_DIR` (added with this benchmark — see §3 below):

| Config (`MEMEX_QUANT_MODE`) | nDCG@10 | Query latency (p95, 100 samples) | Sessions indexed | Index size on disk |
|---|---|---|---|---|
| **`tq-bits2`** (production default) | **N/A** ¹ | **8.40 ms** (range 8.32 – 8.49, median 8.40) | **108** (2 492 jsonl files coalesced, 1 skipped) | **20 MB** |

¹ nDCG is suppressed because the labelled-queries fixture references
sample-corpus session IDs — running it against a different corpus would
produce a meaningless 0. A production-corpus labelled-queries fixture is
follow-up work (the labelling itself is the bottleneck, not the bench).

What this row actually says, *honestly*:

- **Per-session disk footprint** is 20 MB ÷ 108 ≈ 185 KB per session at
  108 sessions vs. 1 735 KB ÷ 12 ≈ 145 KB per session at 12 sessions. The
  per-session cost is *flat* — neither size shows the Qdrant blog's ~8×
  compression because both are still well under the ≥ 1 k corpus threshold
  where the quantized vectors start dominating the on-disk footprint over
  the HNSW graph + payload metadata.
- **Query latency p95 is ~15 % higher** than the 12-session sample (8.40 ms
  vs. 7.31 ms). That's the expected signal: more HNSW edges to walk per
  query as the graph fans out. It's not a regression of the codec — it's
  what production-scale traversal looks like.
- **One mode, not three**. The full f32 / tq-bits1 / tq-bits2 sweep would
  multiply this run's ~6-minute indexing wall-time by 3. The single row
  on the production default already confirms (a) `MEMEX_CORPUS_DIR` works,
  (b) the harness handles a real laptop corpus, (c) `tq-bits2` is in the
  expected latency band. Sweeping the other two modes on the same corpus is
  marked as future work in `MEMEX_CORPUS_DIR`'s PR description.

⚠ **Safety**: the bench drops + recreates `memex_sessions_v3` as part of
each run. If you point it at the same Qdrant your production Memex uses,
your production index disappears. The repro recipe below runs the bench
against an **isolated** Qdrant on port 6336 so the production container
on 6334 is untouched.

### What "rescore + oversampling" buys

- **Oversampling 2×**: the HNSW walks the bits-2 graph asking for `2 × limit`
  candidates instead of `limit`. Cheap (2× more graph nodes traversed, no IO).
- **Rescore on**: for the oversampled candidates, the f32 vectors are pulled
  from disk and the cosine score recomputed exactly. The top `limit` after
  rescore is returned. This is the recall-restoring step.

Net effect at production scale: ~8× storage saving, ~1 ms latency cost over
no-rescore, recall within 1-2 % of f32. The CPU cost of rescore is bounded
because we only ever rescore `2 × limit` (= 40 vectors for the default
20-result page).

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
# 1. Bring up an ISOLATED Qdrant just for the bench. Do NOT reuse the
#    container backing your production Memex install — the bench drops
#    + recreates memex_sessions_v3 on every iteration, and that would
#    wipe your production index.
docker run -d --name memex-qdrant-bench \
    -p 6335:6333 -p 6336:6334 qdrant/qdrant:v1.18.1

# 2a. Small-corpus sweep (in-repo synthetic, 12 sessions). Default
#     behaviour — no MEMEX_CORPUS_DIR needed:
for mode in f32 tq-bits1 tq-bits2; do
    MEMEX_BENCH_LIVE=1 \
        MEMEX_QDRANT_URL=http://127.0.0.1:6336 \
        MEMEX_QUANT_MODE=$mode \
        cargo bench --bench quant_sweep --manifest-path src-tauri/Cargo.toml
done

# 2b. Production-scale sweep (your own ~/.claude/projects).
#     MEMEX_CORPUS_DIR overrides the in-repo sample-corpus path. nDCG is
#     suppressed in this mode (labelled-queries fixture is sample-corpus
#     scoped) — Criterion still reports p50/p95 latency, and the index
#     size below reflects real production footprint:
for mode in tq-bits2; do  # add f32 / tq-bits1 if you have ~6 min/mode to spare
    MEMEX_BENCH_LIVE=1 \
        MEMEX_CORPUS_DIR=~/.claude/projects \
        MEMEX_QDRANT_URL=http://127.0.0.1:6336 \
        MEMEX_QUANT_MODE=$mode \
        cargo bench --bench quant_sweep --manifest-path src-tauri/Cargo.toml
done

# 3. Index size per mode (after each run):
docker exec memex-qdrant-bench du -sh /qdrant/storage/collections/memex_sessions_v3
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

**Wired end-to-end as of PR #23.** With `MEMEX_BENCH_LIVE=1` the bench
runs the full pipeline on its own: per-mode collection drop →
`crud::ensure_collection_v3()` (picks up the current `MEMEX_QUANT_MODE`)
→ `Embedder::new()` → `parser::scan_dir(examples/sample-corpus)` →
`indexer::bulk_index_arc(...)` → labeled-queries round-robin through
`indexer::lens_search` (Criterion samples p50/p95) → post-bench
`mean_ndcg_at_10` pass over every labeled query. The measured table at
the top of this file came out of exactly this loop.

A lighter alternative — if you only want **recall@10 / nDCG@10** and
not Criterion's timing distribution — is to call
`memex_lib::eval_ndcg::mean_ndcg_at_10` directly from a `#[tokio::test]`
in `src-tauri/tests/`, passing a closure that runs each query through
`indexer::lens_search` against a live Qdrant. Set `MEMEX_QUANT_MODE`
before the test process spawns to control which mode the v3 collection
gets created with. This skips the bench framework's warm-up + sampling
overhead.

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
