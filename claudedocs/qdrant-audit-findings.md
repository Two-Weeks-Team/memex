# Memex × Qdrant — 1st-pass AUDIT findings (locked baseline)

**Generated**: 2026-05-28 (this session)
**Purpose**: lock the ground truth of what the code actually does, so the next session's `/goal` starts from FACTS, not assumptions. The goal will re-verify this audit before any implementation.

**Inputs**: `grep -nE …` runs against `src-tauri/src/{schema,lens,indexer,retrieval,crud,wrapped,web}.rs`, `src/main.js`, `~/.cargo/registry/src/index.crates.io-…/qdrant-client-1.18.0/`.

---

## §1 · Pinned versions (FACT)

| Component | Version | Source |
|---|---|---|
| Qdrant server | **v1.18.1** | `deploy/web/docker-compose.yml` — `image: qdrant/qdrant:v1.18.1` |
| Qdrant Rust client | **1.18.0** (resolved) | `Cargo.toml: qdrant-client = "1"` · resolved in `Cargo.lock` |
| Facet API in client | **available** | `~/.cargo/registry/src/.../qdrant-client-1.18.0/tests/snippet_tests/test_facets.rs` exists — SDK ships `Facet` builders |

---

## §2 · What is ACTUALLY wired and ON by default (FACT)

This is the headline. Many features I assumed were "wired-but-dormant" are actually **active in `LensWeights::default()`**.

### `LensWeights::default()` (the truth, `src-tauri/src/lens.rs:132-145`)

```rust
fn default() -> Self {
    Self {
        content:      1.0,   // dense `content` lens ON
        tool:         1.0,   // dense `tool` lens + path_sparse / tool_sparse share this — sparse ON
        path:         1.0,   // dense `path` lens + path_sparse share — sparse ON
        error:        1.0,   // dense `error` lens ON
        code:         1.0,   // dense `code` lens ON
        content_late: 0.0,   // ColBERT multivector lens OFF (no graph build cost paid; rerank-only)
        diversity:    None,  // MMR off (opt-in)
        fusion:       FusionMode::Formula,  // server-side Formula with recency decay
    }
}
```

### `active_dense_specs` (`lens.rs:418`)

Adds a `DenseSpec` for every dense lens whose weight > 0. With defaults: **5 dense lanes active** (content, tool, path, error, code) and `content_late` skipped.

### `active_sparse_specs` (`lens.rs:425-435`)

```rust
fn active_sparse_specs(w: &LensWeights) -> Vec<SparseSpec> {
    let mut v = Vec::with_capacity(2);
    if w.path > 0.0 {
        v.push(SparseSpec { name: "path_sparse", weight: w.path, source_field: "path" });
    }
    if w.tool > 0.0 {
        v.push(SparseSpec { name: "tool_sparse", weight: w.tool, source_field: "tool" });
    }
    v
}
```

→ With defaults (`path:1.0`, `tool:1.0`), **both sparse lanes are ALSO active** by default. The `path` and `tool` weight sliders drive BOTH the dense AND sparse counterparts.

### `build_prefetches` (`lens.rs:686-720`)

```rust
for spec in dense {
    let q: Query = if spec.name == "content" && weights.diversity.is_some() {
        Query::new_nearest_with_mmr(...)
    } else if spec.name == "content_late" {
        Query::new_nearest(VectorInput::new_multi(vec![qvec.to_vec()]))
    } else {
        Query::new_nearest(VectorInput::new_dense(qvec.to_vec()))
    };
    ...
}
for spec in sparse {
    if qsparse.indices.is_empty() { continue; }   // skip empty sparse queries
    let q = Query::new_nearest(VectorInput::new_sparse(...));
    ...
}
```

Both dense and sparse lanes go into the **same prefetch chain**. The top-level query uses `FusionMode::Formula` (default) — which means **server-side Formula scoring fuses 5-dense + 2-sparse prefetches in ONE round trip**.

### Conclusion (FACT)

By default, on every search the code executes against the embedded Qdrant:
- **5 dense BGE-small named-vector prefetches** (content, tool, path, error, code)
- **2 sparse IDF-modified prefetches** (path_sparse, tool_sparse)
- Fused server-side via **`Query::new_formula(...)` with `exp_decay` recency**
- Plus optional MMR diversity when `content` lens is the focus
- Plus optional `Query::RelevanceFeedback` when 👍/👎 buttons are tapped

That is **a hybrid (dense + sparse) retrieval with server-side fusion + recency decay + opt-in MMR + opt-in relevance feedback** — running every time. **The landing currently advertises NONE of this**, except a small note on `Σ(s·w)/Σw`.

---

## §3 · What is WIRED but OFF by default (FACT)

| Feature | Wired location | Default state | How to turn on |
|---|---|---|---|
| `content_late` ColBERT MaxSim | indexed via `indexer.rs:621-625`; queried via `lens.rs:419-420, 691` | `LensWeights::default().content_late = 0.0` (off) | bump default to e.g. 0.3, OR slider in UI |
| MMR diversity reranking | `lens.rs:687-693` `Query::new_nearest_with_mmr` | `LensWeights::default().diversity = None` (off) | set `diversity = Some(0.4)` in UI / weights |
| `Query::RelevanceFeedback` | backend in `retrieval.rs::add_feedback`, frontend buttons in `src/main.js:2894-2895` | OFF — only fires when user clicks 👍/👎 | user-driven, no change needed |
| `FusionMode::Rrf` (RRF alternative to Formula) | `lens.rs` (FusionMode enum) | `FusionMode::Formula` is default | UI toggle (not exposed yet) |

---

## §4 · What is GENUINELY missing (FACT)

| Feature | Where it should live | Why it matters |
|---|---|---|
| **Facets API** | `wrapped.rs` currently uses `scroll_window` + client-side tally; SDK 1.18.0 exposes `Facet` builders (confirmed at `~/.cargo/registry/src/.../qdrant-client-1.18.0/tests/snippet_tests/test_facets.rs`) | Wrapped report assembly is O(N) scroll right now; Facet is O(field cardinality). Big win on 1000+ session corpora. |
| **Prometheus `/metrics` endpoint** | `web.rs:111-135` only has `/api/health` | Server variant has no operational observability. Adding 6-10 Prom metrics is ~40 lines. |
| **Custom text-index tokenizer** | `schema.rs:246` `("ai_title", FieldType::Text, None)` — no tokenizer config | Default tokenizer doesn't split `camelCaseIdentifiers` or `snake_case`. Code-aware splitting would help title search. Need to research what `Text` field index params Qdrant 1.18 exposes via the SDK. |

---

## §5 · Indexed payload fields (FACT, `schema.rs:241-253`)

Goes beyond what the landing claims (4) — actually **10 indexed fields**:

```rust
("project_name",    FieldType::Keyword, Some(tenant_kw)),       // tenant-flagged keyword
("project_path",    FieldType::Keyword, None),
("git_branch",      FieldType::Keyword, None),
("ai_title",        FieldType::Text,    None),                  // full-text
("start_ts_dt",     FieldType::Datetime,Some(datetime_default)),// datetime
("has_errors",      FieldType::Bool,    None),
("schema_version",  FieldType::Integer, None),
("intent",          FieldType::Keyword, None),                  // enriched
("outcome",         FieldType::Keyword, None),                  // enriched
("source_agent",    FieldType::Keyword, None),                  // KH-01 multi-agent
```

The v2 collection (`indexer.rs:245-251`) has 6 indexed fields (less). v3 is the live schema for new writes.

---

## §6 · Collection-level config (FACT, `schema.rs::build_v3_create_collection`)

| Knob | Value | Confirmed at |
|---|---|---|
| Collection name | `memex_sessions_v3` | `schema.rs:34` |
| Schema version stamp | `3` | `schema.rs:37` |
| Dense vector dim | `384` (BGE-small) | `schema.rs:42` |
| Dense vector names | `["content", "tool", "path", "error", "code"]` | `schema.rs:46` |
| Multivector name | `"content_late"` (MaxSim) | `schema.rs:50, 141` |
| Sparse vector names | `["path_sparse", "tool_sparse"]` with `Modifier::Idf` | `schema.rs:54, 159` |
| Per-vector HNSW (m / ef_construct) | content 24/200 · code 20/150 · error 16/100 · tool & path 12/64 · content_late 0/0 | `schema.rs:74-87, 125-148` |
| Quantization | TurboQuant bits-2, `always_ram: true` | `schema.rs:185-188` |
| Quant search params | `rescore: true`, `oversampling: 2.0`, `ignore: false` | `schema.rs:263-266` |
| Strict mode | `max_resident_memory_percent: 85`, `max_query_limit: 100` | `schema.rs:69-71, 195-200` |
| WAL | `capacity_mb: 32` | `schema.rs:84, 201` |

---

## §7 · What the landing currently advertises (FACT)

Q1 — Named vectors per point (5 dense)
Q2 — Universal Query weighted blend (claims "client-side Σ(s·w)/Σw" — accurate but misses that server-side Formula prefetch is what actually runs)
Q3 — Discovery API (correct, used for Mix & Match)
Q4 — Distance Matrix → MST (correct)
Q5 — Payload filter as HNSW pre-filter (correct, `has_errors` example)
Q6 — Native HTTP snapshots (correct)
Small things 1-6 — uuid_v5 ids, "binary-quantized HNSW per name" (WRONG — TurboQuant bits-2), field-indexed predicates, server-side FormulaQuery, ≤1KB payload, gRPC+HTTP dual transport

**Net coverage**: Landing surfaces **~12 features** out of **~26 active** in code. That is **a 2.2× under-sell** (worse than the previous comparison said because sparse + late-interaction + RelevanceFeedback are now confirmed ACTIVE by default, not dormant).

---

## §8 · Diff between landing claims and code reality

| Landing says | Code says | Action |
|---|---|---|
| "binary-quantized HNSW per name" | TurboQuant bits-2 + 2× oversampling + rescore | **CORRECT** (T1.1) |
| "5 named vectors per point" | 5 dense + 2 sparse + 1 multivector = 8 vector slots | **EXTEND** (T1.2) |
| Q2 blend = "Σ(s·w)/Σw client-side" | Server-side Formula with exp_decay recency + RRF/MMR options | **REWRITE Q2 wording** or add new Q7 |
| "9 tools" (in some old run sections) | 11 tools in `mcp.rs` | already fixed in earlier round (sanity) |
| Q4 "12 MST edges" caption | server returns sampled K-NN pairs; MST built client-side in `petgraph` | **REWRITE caption** (T1.3) |
| "field-indexed predicates" (4 listed) | 10 indexed payload fields | **EXTEND small-things** (T2.5, T2.7, T2.8, T2.9) |
| (no mention of sparse) | `path_sparse` + `tool_sparse` ACTIVE by default | **NEW Q8 card** (T2.2) |
| (no mention of ColBERT) | `content_late` wired, default off, easy on | **NEW Q8 card mentions** (T2.2) + T3.3 turn it on |
| (no mention of feedback) | `Query::RelevanceFeedback` + UI buttons live | **NEW Q-card or T2.13 playground** |
| (no mention of strict mode / WAL / tenant-flag / datetime index / text index / per-vector HNSW / server-side fusion) | All shipped | **EXTEND small-things** (T2.4-T2.12) |

---

## §9 · Audit completeness checklist (for the next session's Phase 0 re-audit)

The next session must, before any code changes, re-verify:

- [ ] **A.** `Cargo.lock` still resolves `qdrant-client` to 1.18.0 (no surprise upgrade)
- [ ] **B.** `qdrant/qdrant:v1.18.1` still pinned in `deploy/web/docker-compose.yml`
- [ ] **C.** `LensWeights::default()` still has `content_late: 0.0` (so we know the activation step is still needed)
- [ ] **D.** `active_sparse_specs` still gates on `w.path > 0` / `w.tool > 0` (so the sparse lanes are still active by default)
- [ ] **E.** `wrapped.rs::scroll_window` still scrolls — no surprise Facets implementation appeared from another branch merge
- [ ] **F.** `web.rs` router still lacks `/metrics`
- [ ] **G.** Frontend (`src/main.js`) still has `relevance_feedback` invoke
- [ ] **H.** Architecture SVG STORE band still says "BINARY-QUANTIZED" (T1.1 target)
- [ ] **I.** Q1 card body still has the same wording
- [ ] **J.** SDK 1.18.0 `Facet` builder still present in `~/.cargo/registry/src/.../qdrant-client-1.18.0/`

If any of A-J have changed since 2026-05-28, the next session must reconcile before proceeding.

---

## §10 · Open questions (no decision needed now)

These are left open for the implementation phase to discover:

1. **What's the right non-zero default for `content_late`?** Probably `0.2-0.3` to keep it as a rerank-only nudge without dominating. Decide via the `eval_ndcg` fixture in `src-tauri/src/eval_ndcg.rs`.
2. **Does Qdrant 1.18 `Text` field index expose tokenizer knobs in the SDK?** If not, document it as a 1.18 limit in `docs/wired-but-dormant.md` and revisit at 1.19+.
3. **Facets fallback path?** If `qdrant-client::Facet` doesn't cover everything `wrapped.rs` aggregates (e.g., per-day error counts), keep a `scroll` fallback for the not-easily-faceted aggregates.

---

## §11 · This audit is the source of truth

For the next session, **`qdrant-improvement-goal.md` §3 (the `/goal` text) explicitly references this doc as its baseline**. Any change in code reality must update §1–§7 first, then the goal text updates accordingly.
