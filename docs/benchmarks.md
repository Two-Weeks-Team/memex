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

## How to reproduce

PR #12 REV-13 (CodeRabbit #6) — accurate recipe.

`src-tauri/src/eval_ndcg.rs` is a **library module**, not a binary. It exposes:
- `pub fn ndcg_at_10(actual: &[String], labels: &[String]) -> f64`
- `pub fn mean_ndcg_at_10<F>(labeled: &[LabeledQuery], mut run_query: F) -> f64`

There is no `cargo run --bin memex -- eval ndcg` subcommand. To produce the
numbers in the table above, drive these functions from a small test or
benchmark harness — either a unit test (`#[test]` in `src-tauri/tests/`)
or `cargo bench` with a `criterion` benchmark.

The quantization knobs are set in **`src-tauri/src/schema.rs::quant_search()`**
(NOT in `eval_ndcg.rs`). They are not CLI-toggleable today — flipping them
requires a source edit + rebuild + index rebuild on the affected collection.

Prerequisites:
- Qdrant `v1.18.1` running (`docker compose up -d qdrant` from repo root)
- Memex desktop or web variant compiled (`cargo build --release`)
- ≥ 100 sessions in `~/.claude/projects` or `~/.codex/sessions`

### Baseline run (the v3 production setting — `bits-2 + rescore + 2× oversampling`)

```bash
# 1. Index the corpus into a fresh v3 collection.
memex index ~/.claude/projects --force-rebuild

# 2. Run a NDCG / latency harness that calls `mean_ndcg_at_10` over a
#    labeled fixture you provide. Example shape:
#
#    #[tokio::test]
#    async fn measure_baseline() {
#        let labeled = load_labeled_fixture("queries.json");
#        let n = mean_ndcg_at_10(&labeled, |q| {
#            let t = Instant::now();
#            let hits = indexer::search_content(&qdrant, &emb, q, 10).await?;
#            ELAPSED.record(t.elapsed());
#            hits.into_iter().map(|h| h.session_id).collect()
#        });
#        eprintln!("ndcg@10 = {n}");
#    }

# 3. Index size on disk:
docker exec memex-qdrant du -sh /qdrant/storage/collections/memex_sessions_v3
```

### Variant runs (manual code edits required)

To produce the other two table rows, edit `src-tauri/src/schema.rs::quant_search()`
on a feature branch and rebuild:

| Variant | Edit in `schema.rs::quant_search()` |
|---|---|
| `bits-2`, **no rescore** (row 2) | set `rescore: false`, `oversampling: 1.0` |
| `f32` baseline (row 1) | remove the `quantization_config` block entirely from `build_v3_create_collection()` (so the collection stores f32) |
| `bits-2 + rescore + 2×` (row 3) | the as-shipped default |

Each variant requires:
1. Edit `schema.rs`
2. `cargo build --release`
3. `memex index … --force-rebuild` (the collection schema changed, so the existing index is stale)
4. Run the NDCG harness from step 2 above
5. Record numbers

A future-work item (not in this PR) is exposing the quant params as a
CLI flag on a debug subcommand so the variant sweep doesn't need source
edits. Until then this is the honest recipe.

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

- Quantization config: `src-tauri/src/schema.rs::quant_config` + `quant_search`
- Eval harness: `src-tauri/src/eval_ndcg.rs`
- Feature tour: [`qdrant-features.md`](./qdrant-features.md) §0–§1
- Dormant capabilities: [`wired-but-dormant.md`](./wired-but-dormant.md) §A
