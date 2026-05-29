# Memex × Qdrant — what we use, what we don't, and why

**Generated**: 2026-05-28 · **Qdrant version pinned**: `qdrant/qdrant:v1.18.1` (`deploy/web/docker-compose.yml`) · `qdrant-client = "1"` (Rust SDK 1.18)

This is the engineer's reality-check: a side-by-side of the Qdrant 1.18 feature surface against what Memex actually exercises in code (`src-tauri/src/{schema,indexer,crud,retrieval,companion}.rs`). It is NOT a sales sheet — it is what `grep` says.

## §0 Headline number

The landing page's **Q1–Q6 + 6 "small things"** describe **12 Qdrant features**. The code actually uses **~24 Qdrant features**. The landing under-sells the implementation by roughly **2×**.

The 12 hidden ones live mostly in `schema.rs` (v3 SOTA collection) and `retrieval.rs` (Formula/RRF/MMR/Prefetch/Discovery/MatrixPairs/RelevanceFeedback). The landing should probably promote 4–6 of them.

---

## §1 The matrix

Legend: **✓** in code · **landing** also surfaced on the page · **✗** not used · **N/A** not applicable to a local-first single-user app.

### Vector storage shape

| Qdrant feature | Status | Where in code | Notes |
|---|---|---|---|
| Dense named vectors | ✓ **landing Q1** | `schema.rs::DENSE_VECTORS` | 5 × 384-d cosine BGE-small (`content`, `tool`, `path`, `error`, `code`) |
| Multivector (ColBERT-style late interaction) | ✓ *hidden* | `schema.rs::MULTIVECTOR_NAME = "content_late"` · `MultiVectorComparator::MaxSim` | Reserved & frozen at `m: 0` (no HNSW links). Wired through `MultiDenseVector::from(chunks)` at upsert. Used as a rerank-only slot pending Phase 4. **Landing doesn't show this.** |
| Sparse vectors | ✓ *hidden* | `schema.rs::SPARSE_VECTORS = ["path_sparse", "tool_sparse"]` | Sparse params with `Modifier::Idf` for IDF-modified BM-ish scoring on path/tool tokens. **Landing doesn't show this.** |
| Single (unnamed) vector | ✗ | — | We never use the bare single-vector form |
| `on_disk` vectors / payload | ✗ | All RAM | Local dataset is small; not worth the latency |
| Custom datatypes (float16/uint8) | ✗ | Default f32 | TurboQuant covers compression instead |

### Quantization

| Qdrant feature | Status | Where | Notes |
|---|---|---|---|
| Binary Quantization (BQ) | ✓ *landing wording is slightly outdated* | The landing says "binary-quantized HNSW" but the v3 collection actually uses **TurboQuant bits-2** | The Q1 card mentions "binary-quantized HNSW per name" but `schema.rs::TurboQuantBitSize::Bits2` + `oversampling: 2.0` + `rescore: true` is the truth on v3. v2 uses BQ. |
| TurboQuant (bits-1 / bits-2) | ✓ *hidden* | `schema.rs:185` `TurboQuantizationBuilder::default().bits(Bits2)` | The actual quantization on v3. Always-in-RAM, 2× oversampling, rescore on. **Worth promoting to a "small thing".** |
| Scalar Quantization (SQ) | ✗ | — | TurboQuant supersedes for our workload |
| Product Quantization (PQ) | ✗ | — | Not worth it at 384-d cosine, our dim is small |

### Indexing & search structure

| Qdrant feature | Status | Where | Notes |
|---|---|---|---|
| HNSW per named vector | ✓ **landing** ("BQ-HNSW per vector") | `schema.rs::HnswConfigDiff` | Confirmed |
| **Per-vector HNSW tuning** | ✓ *hidden* | `schema.rs:118-148` per-lens `m` / `ef_construct` overrides | content m=24/ef=200 · code m=20/150 · error m=16/100 · tool & path m=12/64 · content_late m=0 (no links). **The landing's Q1 understates this — the real config is tuned per lens.** |
| `payload_m` (payload-aware HNSW) | ✓ *implicit* | Global `HNSW_PAYLOAD_M = 16` in `HnswConfigDiff` | Used by the field-filtered queries |
| Field index (keyword) | ✓ **landing** | `crud::ensure_collection_v3` + `indexer.rs::ensure_collection` | `project_name`, `git_branch`, `project_path` |
| **Field index (tenant flag)** | ✓ *hidden* | `schema.rs:229-234` `KeywordIndexParamsBuilder::default().is_tenant(true)` on `project_name` | Tells Qdrant to optimize this field as a tenant partition. **Big perf knob, not surfaced.** |
| Field index (bool) | ✓ **landing Q5** | `has_errors` | The "cheap HNSW pre-filter" example |
| Field index (integer) | ✓ *hidden* | `start_ts` | Used for time-range filters |
| **Field index (datetime)** | ✓ *hidden* | `schema.rs:247` `("start_ts_dt", FieldType::Datetime, …)` via `DatetimeIndexParamsBuilder` | RFC-3339 strings, indexed. **Newer index type, not surfaced.** |
| **Field index (full-text)** | ✓ *hidden* | `indexer.rs:249` `("ai_title", FieldType::Text)` | Lexical search on title. **Not surfaced.** |
| Field index (float / geo / uuid) | ✗ | — | No spatial/uuid filters needed |

### Query primitives

| Qdrant feature | Status | Where | Notes |
|---|---|---|---|
| `QueryPointsBuilder` standard search | ✓ **landing Q2** | `indexer.rs::lens_search`, `recall`, `predict_next_action` | |
| `.using("<name>")` named-vector selection | ✓ **landing Q2** | Everywhere | |
| **Universal Query API (multi-stage prefetch)** | ✓ *hidden* | `retrieval.rs::PrefetchQueryBuilder` | We DO use Qdrant's prefetch chain (not just client-side blend). **Landing Q2 implies client-side blending only — server-side prefetch is the bigger story.** |
| **`Query::Formula` (server-side formula scoring)** | ✓ *hidden* | `retrieval.rs::FormulaBuilder` + `DecayParamsExpressionBuilder` | Per-prefetch recency decay + error boost, server-side. **The landing's "small things" mentions it but it deserves a Q-card slot.** |
| **`Query::Rrf` (Reciprocal Rank Fusion)** | ✓ *hidden* | `retrieval.rs::RrfBuilder` | Alternative fusion mode (`FusionMode::Rrf`). The landing says "NOT RRF" for the lens slider, but RRF IS available as a fallback. |
| **MMR diversity reranking** | ✓ *hidden* | `retrieval.rs::MmrBuilder` + `NearestWithMmr(diversity=d)` | Maximal marginal relevance for result diversity. Default 0.4. **Not surfaced anywhere.** |
| `Query::Discover` (Discovery API) | ✓ **landing Q3** | `indexer.rs::mix_match` | |
| `Query::Recommend` | ✗ | — | We use Discovery for everything — Recommend not needed |
| **`Query::RelevanceFeedback`** | ✓ *hidden* | `retrieval.rs:64` `add_feedback` + `NaiveFeedbackStrategy` | Server-side relevance feedback loop. **Brand new in 1.18, totally hidden in our landing.** |
| `SearchMatrixPairs` (Distance Matrix) | ✓ **landing Q4** | `indexer.rs::topology` | |
| Random sample (`sample`) | ✓ **landing Q4 implicit** | `SearchMatrixPointsBuilder.sample(N)` | |
| `with_payload` / `with_vectors` | ✓ Everywhere | | |
| `score_threshold` | ✗ | — | Not used; we always take top-K |

### Filtering

| Qdrant feature | Status | Where | Notes |
|---|---|---|---|
| `Condition::matches` (keyword / bool) | ✓ **landing Q5** | `recall.rs` | `has_errors=true` |
| `Filter { must, should, must_not }` | ✓ partial | Mostly `must` only | We don't exploit `should`/`must_not` |
| `HasIdCondition` | ✓ *hidden* | `retrieval.rs:43` | "Find points among these N ids" — used for re-ranking known sets |
| `Range` (numeric/datetime) | ✓ *hidden* | `start_ts` queries | |
| Geo filter (bounding-box / radius / polygon) | N/A | — | No spatial data |
| Nested filter | ✗ | — | Payload is flat |
| `is_empty` / `is_null` | ✗ | — | Not needed |

### CRUD & batch

| Qdrant feature | Status | Where | Notes |
|---|---|---|---|
| `UpsertPoints` (batch) | ✓ landing implicit | `indexer.rs::index_session` | `wait(true)` per session |
| `GetPoints` (by id) | ✓ | `crud::dual_get_session_payload` | |
| `SetPayload` (merge) | ✓ *hidden* | `crud.rs:215` `SetPayloadPointsBuilder` | Payload-only updates without re-embedding |
| `ScrollPoints` (paginated) | ✓ *hidden* | `crud.rs:325` v2→v3 migration | |
| `count_points` | ✓ *hidden* | `crud.rs:431` | |
| `DeletePoints` | ✗ | — | Never delete (sessions append-only) |
| `UpdateVectors` (vector-only update) | ✗ | — | We re-upsert the whole point |
| `OverwritePayload` | ✗ | — | Always merge |

### Order / Group / Facet

| Qdrant feature | Status | Where | Notes |
|---|---|---|---|
| `OrderBy` | ✓ *hidden* | `retrieval.rs:217 .order_by(proto)` | Recency ordering on `start_ts`. Used by `list_sessions_ordered`. |
| **`group_by` (server-side grouping)** | ✓ *hidden* | `retrieval.rs:241-263` `GroupBy` | KA-03 — lens search with optional grouping. **Lets you do "top result per project" in one query.** Worthy of a landing mention. |
| Facets (`FacetRequest`) | ✗ | — | Could compute project counts faster; not used |

### Snapshots & persistence

| Qdrant feature | Status | Where | Notes |
|---|---|---|---|
| Collection snapshot (`POST /snapshots`) | ✓ **landing Q6** | `indexer.rs::snapshot_export` | |
| Download / restore (`GET`, `POST upload`) | ✓ **landing Q6** | `snapshot_import` | |
| Full snapshot (all collections) | ✗ | — | We only ship one collection |
| Storage snapshot (raw disk) | ✗ | — | Use collection snapshots |
| **Snapshot signing (over-the-wire integrity)** | ✓ (app-side, not Qdrant) | `src-tauri/src/snapshot.rs::SignedEnvelope` | Memex wraps the Qdrant snapshot in its own signed envelope. **Probably worth a Q-card.** |

### Collection-level features

| Qdrant feature | Status | Where | Notes |
|---|---|---|---|
| `CreateCollection` with named vectors map | ✓ | `schema.rs::build_v3_create_collection` | |
| **Strict mode** (`StrictModeConfigBuilder`) | ✓ *hidden* | `schema.rs:195-200` | `max_resident_memory_percent: 85` + `max_query_limit: 100` — protects the embedded Qdrant from OOM by misbehaving clients. **Operationally important, not surfaced.** |
| **WAL config** | ✓ *hidden* | `schema.rs:201` `WalConfigDiff { capacity_mb: 32 }` | Write-ahead-log tuning |
| **Quantization search params** (`oversampling`, `rescore`) | ✓ *hidden* | `schema.rs:191-192` `oversampling: 2.0, rescore: true` | Every search uses these |
| `optimizers_config` (vacuum threshold, indexing threshold) | ✗ default | — | Defaults are fine |
| **Schema versioning payload field** | ✓ *hidden* | `schema.rs::SCHEMA_VERSION_V3 = 3` stamped on every point | Lets us migrate without breaking old reads (`crud::dual_get_session_payload`). **Memex pattern, not Qdrant per se.** |
| Collection aliases | ✗ | — | We hot-cut from v2 to v3 by dual-read instead |
| Sharding | N/A | — | Single node |
| Replication | N/A | — | Single node |
| Write consistency factor | N/A | — | Single node |

### Distance & metrics

| Qdrant feature | Status | Notes |
|---|---|---|
| `Distance::Cosine` | ✓ **landing** | All 5+1+2 vectors |
| `Distance::Euclid` / `Dot` / `Manhattan` | ✗ | Cosine is right for BGE-small |

### Transport / auth / ops

| Qdrant feature | Status | Notes |
|---|---|---|
| gRPC client (`qdrant-client = "1"`) | ✓ landing implicit | All queries |
| REST client (`reqwest`) | ✓ landing Q6 | Snapshot endpoints only |
| Cluster API / peers | N/A | Single node |
| API key auth | ✗ | Local loopback, not needed |
| JWT auth | ✗ | Same |
| TLS | ✗ | Loopback only |
| Telemetry / Prometheus `/metrics` | ✗ | Could expose for the server variant — not done |
| Web UI / dashboard | N/A from Qdrant side; Memex ships its own browser UI on `:8765` |

---

## §2 Twelve hidden features that *deserve* a landing mention

These are features the **code uses** but **Q1–Q6 doesn't surface**. In rough order of "would impress a Qdrant judge":

| Rank | Feature | One-line value | Suggested landing slot |
|---|---|---|---|
| **1** | **Server-side Formula query with prefetch + recency decay** | "Prefetches carry per-vector recency decay, applied server-side — no client round-trip to re-score." | Already in "small things"; **promote to a new Q7** |
| **2** | **TurboQuant bits-2 + 2× oversampling + rescore** | "Server-side bits-2 quantization with rescore, 2× oversampling guard. The compression is real but accuracy holds." | Replace "binary-quantized HNSW" line in Q1 |
| **3** | **Per-vector HNSW tuning** | "content m=24 / code m=20 / error m=16 / tool & path m=12. Each lens gets its own graph density." | New "small thing" or expand Q1 |
| **4** | **`RelevanceFeedback` query type** | "Server-side relevance feedback loop — the model learns from your taps without round-tripping." | Standalone Q-card (it's new in 1.18) |
| **5** | **Multivector (`content_late`, MaxSim)** | "A late-interaction ColBERT-style slot is reserved for Phase 4 reranking — wired but frozen." | Honest "what's next" note |
| **6** | **Sparse vectors with IDF modifier** | "`path_sparse` + `tool_sparse` with the `Idf` modifier turn lexical token matches into a first-class lane." | New Q-card; pairs with the lens slider for hybrid dense+sparse |
| **7** | **Server-side `group_by`** | "One query returns top-K per project — no client-side bucketing." | Mention in Q2 or new "small thing" |
| **8** | **MMR diversity reranking** | "Built-in MMR (default diversity .4) — results stay non-redundant without re-querying." | "Small thing" |
| **9** | **Strict-mode resource limits** | "Server-enforced 85% RAM cap and 100-point query cap — the embedded Qdrant can't OOM your laptop." | Move from internal note to the **Safety** section on the landing |
| **10** | **Tenant-flagged keyword index** | "`is_tenant: true` on `project_name` — Qdrant optimizes the field as a partition key." | "Small thing" |
| **11** | **Datetime payload index** | "`start_ts_dt` indexed via `DatetimeIndexParamsBuilder` — recency queries are first-class." | "Small thing" |
| **12** | **`Filter` `HasIdCondition` + `SetPayload`** | "Re-rank known sets without a full search; update payload without re-embedding." | Optional |

---

## §3 What Qdrant offers that Memex **doesn't** use

### Won't use (by design)

These are correct *no's* for a local-first single-user app. Listing them just so we're explicit:

- **Sharding · Replication · Write-consistency factor** — single-node embedded
- **API key / JWT / TLS auth** — loopback only; the server variant uses Caddy in front for that
- **Geo filters** — no spatial data in coding sessions
- **Recommend API** — Discovery (with positive+negative pairs) is strictly more expressive; we never need plain Recommend
- **Collection aliases** — we hot-cut v2→v3 via dual-read, not by aliasing
- **Optimizers tuning** (vacuum, indexing threshold) — defaults are correct for our write pattern
- **`Distance::Euclid` / `Dot` / `Manhattan`** — cosine is right for BGE-small

### Could use later

Reasonable candidates if Memex grows:

| Feature | Where it'd help |
|---|---|
| **Facets** | Faster project-distribution stats for the Wrapped report (we currently scroll + tally client-side) |
| **`Query::Recommend` with `RecommendStrategy::AverageVector` or `BestScore`** | Alternative to centroid-mix — Qdrant computes the recommendation strategy server-side |
| **Prometheus `/metrics` on the server variant** | Operational visibility for the Docker image — easy add |
| **`UpdateVectors` (re-embed without re-upsert)** | If we ever switch the embedder, we can swap one vector at a time |
| **Cluster `peers` API + sharding** | Day Memex hosts other people's corpora — far away |
| **`text` index custom analyzer** (tokenizer/stemmer config) | The `ai_title` text index uses defaults; could tune for code identifiers |
| **`payload_storage_type: in_memory`** | Already implicit since payload is tiny; would let us tune for larger payloads if added |

### New in Qdrant 1.18 that we *did* adopt (worth flagging)

- **TurboQuant** — adopted, hidden on landing
- **`RelevanceFeedback`** — adopted, hidden on landing
- **`is_tenant` keyword index** — adopted, hidden on landing
- **Datetime index** — adopted, hidden on landing
- **Strict-mode** — adopted, hidden on landing

We track the 1.18 surface aggressively; the landing just doesn't admit it.

---

## §4 Suggested landing update

If we wanted to honestly mirror what the code does, the Qdrant section would grow from **Q1–Q6 + 6 bullets** to **Q1–Q8 + 9 bullets**:

- **New Q7 — Server-side scoring (Formula + RRF + MMR)**: prefetch chains, recency decay, error boost, diversity rerank — all on the server.
- **New Q8 — Hybrid dense + sparse (with IDF) + late-interaction (ColBERT-style)**: 5 dense + 2 sparse + 1 multivector slot.

And the "small things" bullets would gain:
- TurboQuant bits-2 + 2× oversampling + rescore (replaces the BQ line)
- Per-vector HNSW tuning (`m`/`ef_construct` per lens)
- Server-side `group_by`
- Tenant-flagged keyword index
- Datetime + text payload index
- Strict-mode resource caps
- `SetPayload` (no-re-embed updates) + `HasIdCondition` (known-set re-ranking)

Equivalent text could be a "What we use that we don't show on the landing — yet" callout, or these could be folded into the existing cards. Either way the **2× under-sell is the headline finding**.

---

## §5 References

- Code (truth): `src-tauri/src/{schema,indexer,crud,retrieval,companion}.rs`
- Qdrant version pin: `deploy/web/docker-compose.yml` → `qdrant/qdrant:v1.18.1`
- Qdrant client SDK: `src-tauri/Cargo.toml` → `qdrant-client = "1"`
- Qdrant docs concept index: <https://qdrant.tech/documentation/concepts/>
- Landing surfacing: `index.html` § `#qdrant` (Q1–Q6 + small-things bullets)
- Landing engineer's tour: `docs/qdrant-features.md` (covers 5 of the 6 landing Q's)
