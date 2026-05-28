# Memex × Qdrant — wired-but-dormant features

An honest status board of every Qdrant 1.18 / Rust SDK 1.18.0 capability we've
*wired into the v3 schema or retrieval code* but haven't fully activated by
default yet. The point: a future reader (or a teammate three months from now)
should never wonder "is this real or is it landing copy?".

**Generated**: 2026-05-28 (initial), tracked in `qdrant-improvement-goal.md` §2.

Each row:
- **Status flag**: `wired:on` (active by default) · `wired:off` (built into the
  schema/code path but gated off) · `not-wired` (Qdrant feature we have not
  pulled in yet)
- **Where**: code path
- **Activation**: what needs to flip
- **Rationale**: why this is the current setting

---

## A · `wired:on` — running on every query by default

These need no further work; they're already part of the production query plan.

| Capability | Where | Notes |
|---|---|---|
| **5 dense named vectors** (`content`, `tool`, `path`, `error`, `code`) | `schema.rs::VECTORS` · `lens.rs::active_dense_specs` | Each at 384-d cosine BGE-small, with per-vector HNSW tuning. |
| **2 sparse vectors** (`path_sparse`, `tool_sparse`) with `Modifier::Idf` | `schema.rs::SPARSE_VECTORS` · `lens.rs::active_sparse_specs` | Server-side IDF — no client-side TF-IDF state. Ride on `w.path > 0` / `w.tool > 0` gates. |
| **TurboQuant bits-2 + 2× oversampling + rescore** | `schema.rs::quant_config` + `schema.rs::quant_search` | `always_ram: true`, `rescore: true`, `oversampling: 2.0`. The compression is real; the accuracy holds. |
| **Per-vector HNSW tuning** | `schema.rs` HnswConfigDiff per vector name | `content m=24/ef=200` · `code m=20/150` · `error m=16/100` · `tool & path m=12/64`. |
| **Server-side `Query::new_formula`** with `exp_decay` recency | `lens.rs` (default `FusionMode::Formula`) · `retrieval.rs` | The default fusion across the prefetch chain. |
| **`content_late` — ColBERT MaxSim multivector rerank** (PR #12 REV-16 promotion) | Schema: `schema.rs` (multivector slot) · indexed at `indexer.rs:621-625` (token-level vectors). Query path: `lens.rs::build_prefetches` emits `Query::new_nearest(VectorInput::new_multi(...))` when weight > 0. | T3.3 flipped both `lens::LensWeights::default()` and `indexer::LensWeights::default()` from `0.0` to **`0.25`** — a rerank-only nudge that doesn't dominate the dense lenses. HNSW for this slot still has `m: 0, ef_construct: 0` (rerank-only, no graph cost). Rollback path: set both defaults back to `0.0`. |
| **Tenant-flagged `project_name`** keyword index | `schema.rs` payload index list | `is_tenant: true` — Qdrant 1.18 partitions the field as a tenant key. |
| **`Datetime` index on `start_ts_dt`** | `schema.rs` payload index list | Recency queries are first-class via `DatetimeIndexParamsBuilder`. |
| **`Text` index on `ai_title`** | `schema.rs` payload index list | Lexical search on session titles. T3.4 investigates tokenizer customisation. |
| **`Bool` index on `has_errors`** | `schema.rs` payload index list | Powers the proactive recall pre-filter. |
| **`Keyword` indices on `intent`, `outcome`, `source_agent`** | `schema.rs` payload index list | Enriched after each session is summarised. |
| **`Query::RelevanceFeedback`** | `commands.rs::relevance_feedback` · `web.rs::"relevance_feedback"` · `src/main.js::applyRelevanceFeedback` | 👍/👎 buttons on result cards POST positive/negative session-id sets; the next query in-session uses `Query::RelevanceFeedback`. User-driven, no default change needed. |
| **`SearchMatrixPairs`** for topology | `indexer.rs::topology` | One round-trip K-NN graph → `petgraph::min_spanning_tree`. |
| **`DiscoverInput` + `ContextInput`** for Mix & Match | `indexer.rs::mix_match` · `retrieval.rs::discover` | Multi-pair discrimination, server-side. |
| **`HasIdCondition` re-rank** | `retrieval.rs` | Cheap personalisation: known-set re-rank without a full search. |
| **`SetPayload` (payload-only updates)** | `indexer.rs::enrich` | Updates intent/outcome/source_agent without re-embedding. |
| **`OrderBy` recency** | recents panel · server-sorted | `OrderBy { start_ts_dt, DESC }`. |
| **Snapshot HTTP endpoints** | `indexer.rs::snapshot_*` | `reqwest`-wrapped POST/GET/POST-upload. |
| **Strict-mode caps** | `schema.rs::StrictModeConfig` | 85% RAM cap + 100-point query cap. |

---

## B · `wired:off` — built into the schema, gated off by default

These features exist in the v3 collection schema (or the retrieval code path)
but are intentionally not contributing to default queries. Each has an explicit
flip target.

### B.1 · `FusionMode::Rrf` — Reciprocal Rank Fusion alternative

- **Status flag**: `wired:off`
- **Where**: `lens.rs::FusionMode` enum has an `Rrf` variant; the default is
  `Formula`. The retrieval path can swap fusion mode via the enum.
- **Activation**: UI toggle exposing fusion mode (not yet wired to a control).
- **Rationale**: Formula with `exp_decay` recency was the right default for
  the demo (recency matters more than rank position in a session-history
  corpus). RRF is kept as an alternative for diverse-source fusion.
- **Tracked by**: future work (not in current goal).

### B.2 · `Mmr` diversity — opt-in

- **Status flag**: `wired:off`
- **Where**: `lens.rs::LensWeights::diversity: Option<f32>`; default `None`.
  `build_prefetches` emits `Query::new_nearest_with_mmr(...)` for the
  `content` lens *if* `diversity = Some(λ)`.
- **Activation**: UI control or `LensWeights::default().diversity = Some(0.4)`.
- **Rationale**: MMR is great for "diverse results" UX but hurts the "near
  duplicates first" intuition for recall — we want both intuitions available,
  not one as a permanent default.
- **Tracked by**: future work.

---

## C · `not-wired` — Qdrant 1.18 features we have not pulled in

These exist in the SDK / server but we haven't added them to the schema or
retrieval code yet. Each has a 1-line rationale.

| Feature | Why not yet |
|---|---|
| `Query::Recommend` (`AverageVector`, `BestScore` strategies) | Discovery API already covers "more like this batch". Recommend is a redundant entry point for our UX. Revisit if Discovery's target-required constraint becomes a pain point. |
| `UpdateVectors` (swap vector for a point in place) | Useful only if we change embedder. Right now `index_session` rebuilds the point. If we add a 1024-d model alongside BGE-small-384, this becomes the rolling-upgrade path. |
| Collection aliases (blue-green migrations) | Hackathon scale: not needed. At 100k+ session corpora when v3→v4 ships, aliases let us flip without dual-write. |
| Multi-collection (one collection per macro-topic) | One `memex_sessions_v3` collection has been enough. Cross-collection search lands in 1.19. Revisit then. |
| Geo filter | We don't index "where I was when I wrote this". If we ever do (e.g., laptop-on-trip filtering), this is one payload-index line. |
| Optimizer tuning (vacuum / indexing threshold knobs) | The defaults handle the hackathon corpus fine. Revisit at 1M+ points. |
| Custom analyzer (camelCase / snake_case tokenizer for `ai_title`) | **T3.4 RESEARCH DONE — DOCUMENTED 1.18 LIMIT.** SDK 1.18.0 `qdrant_client::qdrant::TextIndexParams` exposes `tokenizer: TokenizerType` with four variants: `Prefix`, `Whitespace`, `Word`, `Multilingual`. None split `camelCaseIdentifiers` or `snake_case` — `Word` treats `getUserData` as a single token. Other SDK knobs ARE available (`lowercase` · `min_token_len` · `max_token_len` · `stopwords` · `phrase_matching` · `stemmer` · ASCII folding), but the code-identifier split we want is not yet a 1.18 primitive. **Workaround paths**: (1) client-side pre-tokenization at index time (cheap), (2) wait for Qdrant 1.19+ to expose a `code` tokenizer or a regex-based splitter. Revisit on next Qdrant minor. Source: `~/.cargo/registry/src/.../qdrant-client-1.18.0/src/qdrant.rs:1180-1205` + `:2096-2120`. |
| API key / JWT / TLS auth on the embedded Qdrant | We loopback-bind to 127.0.0.1 in `docker-compose.yml` ("THR-06") so this is defence-in-depth rather than required. Caddy reverse-proxy handles TLS for the all-in-one server variant. |

---

## D · How to keep this doc honest

This doc is meant to drift in only one direction: items move from `wired:off`
or `not-wired` toward `wired:on` as we ship them.

When you flip a flag:
1. Move the row from §B (or §C) into §A.
2. Drop the **Activation** line (it's no longer pending).
3. Keep the **Where** line as the canonical code reference.
4. Update `qdrant-improvement-goal.md` if the change closes a task.

When you add a new feature wired into the schema but off by default, add a
new §B row with all 4 metadata lines populated. Don't ship a wired-but-off
feature without listing it here — otherwise a future reader has no way to
distinguish "real but gated" from "imagined".

---

## E · Reference

- Authoritative code: `src-tauri/src/{schema,lens,indexer,retrieval}.rs`
- v3 collection name: `memex_sessions_v3`
- SDK: `qdrant-client = "1"` resolved to `1.18.0` (see `Cargo.lock`)
- Server pin: `qdrant/qdrant:v1.18.1` (`docker-compose.yml`, `deploy/web/Dockerfile`)
- Related: [`qdrant-features.md`](./qdrant-features.md) · [`benchmarks.md`](./benchmarks.md) · [`architecture.md`](./architecture.md)
