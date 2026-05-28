# Memex × Qdrant — full improvement plan

**Source**: `claudedocs/qdrant-feature-comparison.md` (the diff between code & landing)
**Generated**: 2026-05-28

Five tiers. No scope limit. Each tier groups atomic items so the user can pick.

---

## TIER 1 · Accuracy fixes (MUST — landing currently mis-states reality)

These are corrections, not additions. Doing them is non-negotiable for an "honest copy" landing.

| # | Fix | Where | Effort |
|---|---|---|---|
| 1.1 | **TurboQuant ≠ BQ.** Landing Q1 + architecture STORE panel say "binary-quantized HNSW per vector"; v3 actually uses **TurboQuant bits-2 + 2× oversampling + rescore**. Correct wording everywhere. | `index.html` Q1 card · arch SVG pill `"5 × 384-D · COSINE · BINARY-QUANTIZED HNSW PER VECTOR"` · qd-extras bullet 2 | XS |
| 1.2 | **Architecture STORE band undercounts.** Diagram shows "5 named vectors per point". Reality is 5 dense + 1 multivector (`content_late` MaxSim) + 2 sparse (`path_sparse`, `tool_sparse` with IDF). Update the inline SVG schema panel to show all 8 vector slots; label the multivector + sparse as "wired, Phase-4 / hybrid lane". | arch SVG schema panel | S |
| 1.3 | **`docs/qdrant-features.md` is partially stale.** Describes the v2 collection in some places. Sync to v3 reality (TurboQuant, per-lens HNSW, sparse+multivector slots, Formula prefetch). | `docs/qdrant-features.md` | M |
| 1.4 | **Q-card cross-refs sometimes lie.** Q1's "binary-quantized" line; Q4's caption ("12 MST edges" actually 12 in viz but in code we use a sampled K-NN). Audit each Q card for any claim that doesn't match `src-tauri/src/`. | `index.html` Q1-Q6 | S |

---

## TIER 2 · Landing surfacing (HIGH VALUE — close the 2× gap)

The code uses 24 features; landing shows 12. Promote the most impressive hidden ones.

### 2.A New Q-cards (Q1-Q6 → Q1-Q8)

| # | New card | What it shows | Mini-viz idea |
|---|---|---|---|
| 2.1 | **Q7 — Server-side scoring (Formula · RRF · MMR · prefetch)** | "Prefetches carry per-vector recency decay + error boost, fused server-side; optionally RRF; MMR for diversity. No client round-trip to rescore." | Animated 3-stage pipeline: prefetches → fusion → MMR-diversify |
| 2.2 | **Q8 — Hybrid retrieval (dense + sparse + late-interaction)** | "5 dense BGE lenses · 2 sparse IDF-modified (path_sparse, tool_sparse) · 1 multivector ColBERT (MaxSim) slot. The collection holds them; the query lane fuses them." | Three rails (dense / sparse / multi) feeding into a single ranked result list |

### 2.B Small-things expansion (6 → 12 bullets)

Add to existing 6:

| # | Addition | One-line |
|---|---|---|
| 2.3 | TurboQuant bits-2 + rescore | "2-bit per dim · 2× oversampling · rescore on — the compression is real, the accuracy holds" |
| 2.4 | Per-vector HNSW tuning | "content m=24/ef=200 · code m=20/150 · error m=16/100 · tool & path m=12/64 — each lens gets its own graph density" |
| 2.5 | Server-side `group_by` | "One query returns top-K *per project* — no client bucketing" |
| 2.6 | Tenant-flagged keyword index | "`is_tenant: true` on `project_name` — Qdrant optimizes the field as a partition key" |
| 2.7 | Datetime payload index | "`start_ts_dt` via `DatetimeIndexParamsBuilder` — recency queries are first-class" |
| 2.8 | Full-text payload index | "`ai_title` indexed with `FieldType::Text` — lexical search on session titles" |
| 2.9 | Strict-mode caps | "85% RAM cap + 100-point query cap — embedded Qdrant can't OOM your laptop" |
| 2.10 | `SetPayload` (no-re-embed updates) | "Payload-only updates skip the embedder — fast metadata edits" |
| 2.11 | `HasIdCondition` re-ranking | "Re-rank a known set without a full search — cheap personalization" |
| 2.12 | `OrderBy` recency lane | "`OrderBy { start_ts, DESC }` for the recents panel — server-sorted" |

### 2.C New interactive playground

| # | Playground | Where |
|---|---|---|
| 2.13 | **Relevance-feedback live demo** — 5 mock result cards, drag 👍 / 👎 to mark positive/negative, see Qdrant's `RelevanceFeedback` payload + ranks update | new Q-card row after Q2 |
| 2.14 | **Hybrid lane visualizer** — 3 toggles (dense / sparse / late), each turns on a lane; result list re-orders | inside Q8 |

---

## TIER 3 · Engine improvements (ACTIVATE WHAT'S WIRED-BUT-DORMANT)

These are real Rust changes — actual code, build, tests.

### 3.A Activate hybrid retrieval (sparse + dense fusion)

| # | Change | Effort | Risk |
|---|---|---|---|
| 3.1 | **Audit**: confirm whether `path_sparse` / `tool_sparse` are actually QUERIED by `retrieval.rs::run_query` — if only INDEXED but never used in prefetch, wire them in the existing `FusionMode::Formula` chain with a small weight. | M | low |
| 3.2 | Add a `--hybrid` flag (or auto-enable when sparse non-empty) so the lens slider can include path_sparse/tool_sparse as 6th/7th lanes. | M | low |
| 3.3 | Benchmark: measure recall@10 vs dense-only on a fixture corpus; report numbers in `docs/qdrant-features.md`. | S | low |

### 3.B Activate ColBERT late-interaction (Phase 4)

| # | Change | Effort | Risk |
|---|---|---|---|
| 3.4 | Flip `content_late` from `m: 0` (no HNSW links) to `m: 16` (real graph) once we wire the rerank path. | XS | med (storage growth) |
| 3.5 | Implement a `rerank_with_multivector(results)` step in `retrieval.rs` that takes the top-50 from the dense lane and re-scores with `content_late` MaxSim. | L | med |
| 3.6 | UI: add a "deep rerank" toggle so the user can compare with/without. | M | low |

### 3.C Server-side aggregations (Wrapped via Facets)

| # | Change | Effort | Risk |
|---|---|---|---|
| 3.7 | Replace client-side scroll-and-tally in `wrapped.rs` with Qdrant's **Facet** API for project / branch / intent / arc / outcome distributions. Cuts Wrapped latency by ~10× on large corpora. | M | low |

### 3.D Observability for the server variant

| # | Change | Effort | Risk |
|---|---|---|---|
| 3.8 | Expose Prometheus `/metrics` on the all-in-one container's :8765 (counts: queries/sec, recall poll rate, embedder lock waits, snapshot bytes). | M | low |

### 3.E Custom analyzer for code identifiers

| # | Change | Effort | Risk |
|---|---|---|---|
| 3.9 | Investigate Qdrant 1.18's text-index `tokenizer` knob (camelCase / snake_case splitter) for `ai_title`. If supported, configure; otherwise document the limit. | S (research) → M (impl) | low |

### 3.F User-facing relevance feedback loop

| # | Change | Effort | Risk |
|---|---|---|---|
| 3.10 | Thumbs-up / thumbs-down icons on each search result card in the desktop UI. Each click POSTs a `FeedbackItem` server-side; subsequent queries in the same session use `Query::RelevanceFeedback` to bias. | L | med (UX design) |

---

## TIER 4 · Future-looking (worth considering, not urgent)

| # | Item | Rationale |
|---|---|---|
| 4.1 | `Query::Recommend` with `AverageVector` + `BestScore` strategies | Alternative to Discovery for "more like this batch" UX |
| 4.2 | `UpdateVectors` API | Lets us swap embedder (e.g., to a 1024-d model) one vector at a time without re-upserting payload |
| 4.3 | Collection aliases | Blue-green migration v3→v4 without dual-read in client |
| 4.4 | Multi-collection per "knowledge graph" | One memex collection per macro-topic instead of one giant one |
| 4.5 | Geo filter | If we ever index "where I was when I wrote this" |
| 4.6 | Optimizer tuning (vacuum / indexing threshold) | If the local corpus grows to 100k+ sessions |

---

## TIER 5 · Documentation & communication

| # | Change |
|---|---|
| 5.1 | Sync `docs/qdrant-features.md` to v3 reality (TurboQuant · per-lens HNSW · sparse · multivector slots · Formula prefetch). |
| 5.2 | Add `docs/wired-but-dormant.md` — honest list of features in the schema but not in the query path yet (`content_late` rerank, sparse query lanes). |
| 5.3 | Add `docs/benchmarks.md` — recall@10 / latency-p95 / index-size comparing baseline · TurboQuant · TurboQuant+oversampling. |
| 5.4 | Add 3 sequence diagrams to `docs/architecture.md`: (a) index path, (b) query path (with prefetch + fusion + MMR), (c) snapshot lifecycle. |
| 5.5 | Add an "engineering credibility" subsection on the landing — "Qdrant 1.18 features adopted within 30 days of release: 4". |

---

## Total scope summary

| Tier | Items | Total effort | Risk | Visible? |
|---|---|---|---|---|
| 1 · Accuracy | 4 | S+S+M+S = ~1 day | low | landing & docs |
| 2 · Surfacing | 12 (2 cards + 10 bullets + 2 demos) | ~3 days | low | landing only |
| 3 · Engine | 10 (3.A 3 items, 3.B 3, 3.C 1, 3.D 1, 3.E 1, 3.F 1) | ~2 weeks | mixed | code + landing |
| 4 · Future | 6 | quarterly | future | future |
| 5 · Docs | 5 | ~2 days | low | docs only |

A realistic "do it all properly" pass is Tier 1 + 2 + 3 + 5 — landing perfect, engine activated, docs synced. ~3 weeks for one person, parallelizable.

A minimum-honest pass is Tier 1 alone — that's a one-afternoon fix and stops the landing from mis-stating reality.
