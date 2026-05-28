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
| **`Text` index on `ai_title`** | `schema.rs` payload index list | Lexical search on session titles. Issue #14 added a sibling `ai_title_tokens` field for identifier-aware search; `ai_title` itself remains the display string. |
| **`Text` index on `ai_title_tokens`** — identifier-aware tokenization (Issue #14 promotion) | `schema.rs` payload index list + `schema::identifier_tokens()` helper | Multi-value payload (`Vec<String>`). Each array element is treated as an independent token by Qdrant's Text index. Indexer expands `getUserData refactor` → `["getUserData", "get", "User", "Data", "refactor"]` at upsert time. Same operation as Elasticsearch's `word_delimiter_graph` filter, done client-side. |
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
| **`FusionMode::Rrf`** — RRF as alternative to Formula (Issue #15 promotion) | `lens.rs::FusionMode` enum + `LensWeights::fusion` field | Activated by web/Tauri/MCP callers via JSON `"fusion": "Rrf"`. Issue #15 collapsed `indexer::LensWeights` into `lens::LensWeights` so this knob is now exposed on every external surface. |
| **`Mmr` diversity** — opt-in diversification on `content` prefetch (Issue #15 promotion) | `LensWeights::diversity: Option<f32>` + `build_prefetches` wraps the content prefetch with `Query::new_nearest_with_mmr(d)` when `Some(d)` | Activated by callers via JSON `"diversity": 0.4`. `src/main.js` was already sending this field; Issue #15 makes the backend deserialise it. |

---

## B · `wired:off` — built into the schema, gated off by default

_Currently empty._ The former §B.1 (`FusionMode::Rrf`) and §B.2 (`Mmr` diversity)
were promoted to §A by **Issue #15**: collapsing `indexer::LensWeights` into the
canonical `lens::LensWeights` means the public API now exposes both knobs via
serde JSON, and `src/main.js` was already sending those fields. The activation
gates that were "wired:off" are now closed.

Future `wired:off` items would land here.

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
| ~~Custom analyzer (camelCase / snake_case tokenizer for `ai_title`)~~ | ~~T3.4 RESEARCH DONE — DOCUMENTED 1.18 LIMIT.~~ **Issue #14 (PR landing this work) closes this row** by promoting to §A above with the client-side `identifier_tokens()` helper. The Qdrant SDK 1.18 limit (4 builtin tokenizers, no custom registration) still stands, but it no longer blocks us — splitting at index time into `ai_title_tokens` gives the same retrieval result as a server-side custom tokenizer would. Forking Qdrant or filing an upstream feature request was considered and rejected (timeline uncertainty + double-migration cost). |
| API key / JWT / TLS auth on the embedded Qdrant | We loopback-bind to 127.0.0.1 in `docker-compose.yml` ("THR-06") so this is defence-in-depth rather than required. Caddy reverse-proxy handles TLS for the all-in-one server variant. |
| ~~MCP write tools + `memex_points_indexed_total` increment site~~ | ~~Issue #16 — conditional follow-up.~~ **Issue #16 Stage 2 SHIPPED**: the first MCP write tool — `refresh_session_enrich` — was added in `mcp.rs::tool_call`. It re-runs the deterministic `enrich()` pipeline on one indexed session and writes the result back via `SetPayload` (payload-only, no re-embedding, idempotent). The handler calls `state.mark_indexed(1)` on success; on the HTTP MCP transport this flips `memex_points_indexed_total` off zero (`McpState` gained a `#[cfg(feature = "web")]` optional `Arc<WebMetrics>` field, wired by `web::serve`). On the desktop stdio MCP path it remains a no-op (no `/metrics` endpoint exists on that transport — metric-set uniformity is preserved per Prometheus best practice). |

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
