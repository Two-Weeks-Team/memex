# Memex × Qdrant — feature tour (v3 schema)

This is the engineer's tour of the Qdrant primitives Memex is built on. It is
written against the **v3 collection** (`memex_sessions_v3`) — 5 dense + 2 sparse
+ 1 multivector slots per point, server-side Formula fusion, TurboQuant bits-2
quantization, per-lens HNSW tuning, 10 indexed payload fields. This document is
the *canonical reference* the landing's Qdrant section (`index.html` `#qdrant`)
points at.

**Server pin**: `qdrant/qdrant:v1.18.1` (in `./docker-compose.yml` for the
optional dev container, and in `deploy/web/Dockerfile` for the all-in-one image).
**Rust client**: `qdrant-client = "1"` (resolved to `1.18.0` in `Cargo.lock`).

Each section says:

1. **What you see** — the UI affordance.
2. **What Qdrant does** — the v3 primitive being exercised.
3. **Why it matters** — what a single-vector competitor can't replicate.
4. **Code pointer** — where in this repo.

---

## 0. The v3 collection shape (read this first)

One point per Claude/Codex session, with **8 vector slots**:

| Slot | Kind | Built from | Default state |
|---|---|---|---|
| `content` | dense · 384-d cosine | joined user+assistant prose | ON (weight 1.0) |
| `tool` | dense · 384-d cosine | tool-use names | ON (weight 1.0) |
| `path` | dense · 384-d cosine | file paths touched | ON (weight 1.0) |
| `error` | dense · 384-d cosine | error spans only | ON (weight 1.0) |
| `code` | dense · 384-d cosine | code blocks + identifiers | ON (weight 1.0) |
| `path_sparse` | sparse · IDF modifier | bag-of-paths | ON (rides on `w.path > 0`) |
| `tool_sparse` | sparse · IDF modifier | bag-of-tool-names | ON (rides on `w.tool > 0`) |
| `content_late` | multivector · ColBERT MaxSim | token-level content embeddings | **OFF** by default (`content_late: 0.0` in `LensWeights::default()`) |

Every dense vector is stored under **TurboQuant bits-2** quantization with
`always_ram: true` (in-memory). The HNSW index is per-vector and tuned
individually — see §1.

Search-time params (in `schema.rs::quant_search`): `rescore: true` ·
`oversampling: 2.0` · `ignore: false`. That's the "2× oversampling + rescore"
guard that lets us pay the bits-2 compression cost without losing accuracy.

Authoritative sources in code: `src-tauri/src/schema.rs` (collection shape),
`src-tauri/src/lens.rs` (`active_dense_specs` · `active_sparse_specs` ·
`build_prefetches`), `src-tauri/src/indexer.rs` (`index_session`).

---

## 1. Lens slider — multi-named-vector scoring with weights

**What you see.** Five sliders in the sidebar — `content`, `tool`, `path`,
`error`, `code`. Drag any slider toward 0 to drop that lens; drag toward 2 to
bias toward it. Every result card shows the per-vector contribution as chips
so you can see *which lens earned this hit*.

**What Qdrant does.** One `query()` call carries a **prefetch chain** with one
lane per non-zero weight — by default 5 dense + 2 sparse lanes — fused
server-side via `FusionMode::Formula` with `exp_decay` recency:

```rust
// Pseudocode of build_prefetches in src-tauri/src/lens.rs
for spec in dense {
    prefetches.push(Query::new_nearest(VectorInput::new_dense(qvec))
        .using(spec.name)
        .limit(spec.limit));
}
for spec in sparse {
    if qsparse.indices.is_empty() { continue; }
    prefetches.push(Query::new_nearest(VectorInput::new_sparse(idx, val))
        .using(spec.name)
        .limit(spec.limit));
}
// Top-level: Formula with decay over the prefetch results
QueryPointsBuilder::new("memex_sessions_v3")
    .add_prefetch(prefetches)
    .query(Query::new_formula(formula_with_exp_decay))
    .limit(20)
    .with_payload(true)
```

The conceptual blend `Σ(scoreᵢ · wᵢ) / Σ wᵢ` shown in the landing playground is
the *same* weighted blend the server-side Formula computes — visualised
client-side for clarity, executed server-side in one round-trip.

**Per-vector HNSW tuning** (in `schema.rs::build_v3_create_collection`): each
named vector gets its own graph density:

| Vector | `m` | `ef_construct` |
|---|---|---|
| `content` | 24 | 200 |
| `code` | 20 | 150 |
| `error` | 16 | 100 |
| `tool` | 12 | 64 |
| `path` | 12 | 64 |
| `content_late` | 0 | 0 (no graph — rerank-only) |

The `content` lens (semantic prose) earns the densest graph because it's the
default; sparse/lexical lenses ride on cheaper m/ef.

**Why it matters.** With one vector per session you can't say "weight tool
calls 2× more than prose for this query". You could shoehorn it with RRF, but
RRF is rank-based and ignores weights. Qdrant's *named vectors per point* +
server-side Formula prefetch gives you continuous control AND a single
round-trip.

**Code pointer.** `src-tauri/src/lens.rs` (build_prefetches, active_*_specs) +
`src-tauri/src/indexer.rs::lens_search`.

---

## 2. Mix & Match — Discovery API with context pairs

**What you see.** Click `+ pos` on cards you like, `− neg` on cards you
dislike. Open the Mix & Match modal → Run discovery. You get the sessions
that "smell like" your positives while "smelling unlike" your negatives.

**What Qdrant does.** A `DiscoverInput { target, context: ContextInput }` is
wrapped in a `Query` and sent through `QueryPointsBuilder.query(…)`. Each
`ContextInputPair { positive, negative }` is a vector hint; Qdrant uses them
as a discrimination function to re-rank points around the `target` anchor.

Qdrant 1.18 server requires a non-null `target`, so we set it to the first
positive session — a sane default: "find sessions like *this one*, biased by
these contrastive pairs."

**Why it matters.** Recommendation APIs (find points like Y) and vanilla
searches (find points near Q) can't express "do find X — but only the kind of
X that *isn't* like Z." That's exactly the question you ask when sifting
through 80 sessions for the *next* one you actually want to read.

**Code pointer.** `src-tauri/src/indexer.rs::mix_match` +
`src-tauri/src/retrieval.rs::discover`.

---

## 3. Topology — Distance Matrix + MST

**What you see.** A Topology button opens a modal with a radial SVG. Every
session is a node, colored by project, sized by user-turn count. Lines are
the **minimum spanning tree** edges of the pairwise distance graph — the
"backbone" of similarity across your whole corpus. Click any node → close
modal + load that session in the inspector.

**What Qdrant does.**

```rust
client.search_matrix_pairs(
    SearchMatrixPointsBuilder::new("memex_sessions_v3")
        .using("content")
        .sample(N)              // how many points to consider
        .limit(K),              // K nearest neighbors per sampled point
).await?
```

That returns `SearchMatrixPairs.pairs: Vec<SearchMatrixPair{a,b,score}>` —
top-K nearest neighbors per sampled point, with similarity scores. We feed
those into `petgraph::UnGraph<String, f32>` and call `min_spanning_tree`.

**Why it matters.** Most vector DBs make you build the pairwise matrix
client-side (N² queries) or expose it only via batch search. Qdrant's matrix
endpoint gives you a sampled K-NN graph in one round-trip — exactly what an
MST renderer needs.

**Code pointer.** `src-tauri/src/indexer.rs::topology` +
`src/main.js::renderTopologySvg`.

---

## 4. Replay — on-demand JSONL re-parse

**What you see.** Every result card has a Replay button. Click it: split view
with a turn list on the left and a turn detail on the right, ⏮ ⏯ ⏭ controls,
and a speed selector (1× / 2× / 4× / 8×). Tool calls render as Bash terminals,
Edit diffs, Read snippets, etc. Errors get a red border.

**What Qdrant does.** Nothing exotic — but the *design* leans on Qdrant's
payload system. We index a tiny `source_path` in the payload, so to replay
session X we just call `client.get_points([uuid_v5(X)])`, read
`source_path`, and re-parse the JSONL file on disk. Qdrant payloads stay
small (≤1 KB per session) and the index stays fast.

**Why it matters.** Storing the full transcript in the payload would balloon
the index. Re-parsing on demand is ~100 ms for a 5 000-turn session — well
under what a human notices.

**Code pointer.** `src-tauri/src/commands.rs::get_session_turns` +
`src/main.js::openReplay` / `renderToolCall`.

---

## 5. Proactive recall — `error` named vector + payload filter

**What you see.** While you're working in another Claude Code / Codex session,
Memex polls the session log directory for fresh `tool_result.is_error` events.
When it sees one, it asks Qdrant: *"have I solved a similar error before?"*
If yes, a banner slides into the bottom-right with the past-fix candidate.
Click "Open replay" and you land directly inside the relevant past session.

**What Qdrant does.**

```rust
client.query(
    QueryPointsBuilder::new("memex_sessions_v3")
        .query(embedded_error)        // BGE-small of the error text
        .using("error")               // ← the dedicated error vector
        .limit(5)
        .filter(Filter { must: vec![Condition::matches("has_errors", true)] })
        .with_payload(true),
).await?
```

Two things make this work that wouldn't on a single-vector setup:

- The dedicated `error` named vector is built from *only* the
  `tool_result.is_error=true` content + heuristic "Error:" lines from
  assistant turns. So semantically similar text — but in a happy-path
  session — won't drown out actual past-fix sessions.
- The `has_errors` payload index is keyword/bool so the filter is a cheap
  HNSW pre-filter, not a full collection scan.

**RelevanceFeedback signal** (v3): when you click 👍 (more like this) or 👎
(less) on a result card, the next query in the same session uses
`Query::RelevanceFeedback` to bias the ranking with positive/negative
session-id sets. The buttons live at `src/main.js:~2895`, the Tauri command at
`src-tauri/src/commands.rs::relevance_feedback`, and the HTTP route at
`src-tauri/src/web.rs::"relevance_feedback"` for the Docker server variant.

**Why it matters.** "Have I seen this before?" is the killer feature for an
engineer's session history. Without (a) a dedicated error lens and (b) a
cheap "had errors" filter, the recall feed gets noisy fast.

**Code pointer.** `src-tauri/src/indexer.rs::recall` +
`src-tauri/src/retrieval.rs::relevance_feedback` +
`src/main.js::pollRecall` / `applyRelevanceFeedback`.

---

## 6. Hybrid retrieval — sparse + dense + (opt-in) multivector

**What you see.** Today: invisible. The default search transparently fuses 5
dense lanes + 2 sparse lanes — you don't see a knob for it. T2.14's Hybrid
Lane Visualizer on the landing surfaces the three rails.

**What Qdrant does.** Two sparse vector slots run alongside the 5 dense ones:

```rust
// src-tauri/src/schema.rs (v3)
SparseVectorsConfig {
    map: { "path_sparse": SparseVectorParams { modifier: Idf, .. },
           "tool_sparse": SparseVectorParams { modifier: Idf, .. } }
}
```

`Modifier::Idf` makes Qdrant compute inverse-document-frequency weights
server-side — no client-side TF-IDF maintenance. The sparse lanes share the
`path` / `tool` weight sliders (`lens.rs::active_sparse_specs` gates on
`w.path > 0` and `w.tool > 0` respectively), so toggling a dense lens
toggles its sparse counterpart in lockstep.

The third slot is **`content_late`** — a multivector (list of token-level
embeddings) scored via ColBERT MaxSim. It's indexed via `VectorsConfig`'s
multivector entry (`Multivectors`); the HNSW graph is intentionally **off**
(`m: 0`, `ef_construct: 0`) so it carries no index cost until activated.
`LensWeights::default().content_late` is `0.0` in v3 — opt-in rerank lane.
T3.3 in the goal flips this to a deliberate non-zero (target `0.25`).

**Why it matters.** Three orthogonal scoring signals (dense semantic /
sparse lexical / late-interaction token-level) in one query plan, fused
server-side. Each compensates for the others' blind spots: dense handles
paraphrase, sparse handles exact-identifier matching, late-interaction
handles fine-grained position-aware matching.

**Code pointer.** `src-tauri/src/schema.rs` (SPARSE_VECTORS · MULTIVECTOR_NAME)
+ `src-tauri/src/lens.rs::build_prefetches` (the unified prefetch chain).

---

## 7. Server-side scoring — Formula · Prefetch · RRF · MMR · RelevanceFeedback

**What you see.** Every result list is already MMR-diversified when the
`diversity` knob is set; recency is baked into the score via `exp_decay`
without a client-side re-sort.

**What Qdrant does.** The top-level query in v3 is **`Query::new_formula`** by
default (`FusionMode::Formula`):

```rust
let formula = FormulaBuilder::new()
    .expression(/* recency decay over start_ts_dt */)
    .build()?;
QueryPointsBuilder::new("memex_sessions_v3")
    .add_prefetch(/* 5 dense + 2 sparse lanes */)
    .query(Query::new_formula(formula))
    .limit(20)
```

Alternatives Qdrant 1.18 supports natively (`FusionMode` enum):

- `Rrf` — rank fusion across prefetches
- `Mmr` — diversify by penalising near-duplicates (set `diversity` 0-1)
- `RelevanceFeedback` — bias next query by 👍/👎 from prior query

All of these are server-side. No client round-trip to re-score.

**Why it matters.** The competing approach is to fetch 1 000 raw points then
re-rank in Rust. That hammers IO and burns latency. Server-side Formula folds
fusion + recency + diversity into one query plan; the client only sees the
final top-K.

**Code pointer.** `src-tauri/src/lens.rs::FusionMode` +
`src-tauri/src/retrieval.rs` (the Formula construction).

**Known limitation — web variant API surface** (PR #12 REV-15). The
`POST /api/lens` handler accepts `indexer::LensWeights`, a 6-field struct
covering the dense weights + `content_late` only. `MMR diversity` and
`FusionMode::Rrf` live on the richer `lens::LensWeights` (8 fields) and are
exposed only on the desktop (Tauri) command surface today. A future PR can
either (a) widen `indexer::LensWeights` to mirror the lens module's struct,
or (b) thread `lens::LensWeights` through the web handler directly. Tracked
in `docs/wired-but-dormant.md` §C if needed.

---

## 8. Indexed payload fields — 10 first-class predicates

`schema.rs::build_v3_create_collection` declares **10 indexed payload fields**,
not the 3-4 the v2 era described:

```rust
("project_name",   FieldType::Keyword, Some(is_tenant: true)),  // tenant-flagged
("project_path",   FieldType::Keyword, None),
("git_branch",     FieldType::Keyword, None),
("ai_title",       FieldType::Text,    None),                   // full-text
("start_ts_dt",    FieldType::Datetime, Some(default_params)),  // datetime
("has_errors",     FieldType::Bool,    None),
("schema_version", FieldType::Integer, None),
("intent",         FieldType::Keyword, None),
("outcome",        FieldType::Keyword, None),
("source_agent",   FieldType::Keyword, None),                   // KH-01 multi-agent
```

Highlights:

- **`is_tenant: true` on `project_name`** — Qdrant 1.18 partitions the field
  as a tenant key, so per-project queries narrow before the HNSW walk.
- **`Datetime` on `start_ts_dt`** — built via `DatetimeIndexParamsBuilder`,
  recency queries are first-class. The recents panel uses
  `OrderBy { start_ts_dt, DESC }` and Qdrant returns them server-sorted.
- **`Text` on `ai_title`** — lexical search on session titles runs alongside
  vector search. T3.4 investigates whether the 1.18 SDK exposes a custom
  tokenizer (camelCase / snake_case splitter).

---

## 9. Strict-mode resource caps

Embedded Qdrant runs inside your laptop; we can't let a runaway prefetch OOM
the host. `schema.rs::build_v3_create_collection` sets:

```rust
StrictModeConfig {
    enabled: true,
    max_resident_memory_percent: 85,
    max_query_limit: 100,
    ...
}
```

Plus `WalConfig { capacity_mb: 32 }` to keep the write-ahead log bounded.

---

## Snapshot — boring but essential

Qdrant has built-in snapshot export/import via HTTP:

```
POST /collections/{C}/snapshots          → create + filename
GET  /collections/{C}/snapshots/{name}   → download bytes
POST /collections/{C}/snapshots/upload?priority=snapshot → restore
```

We wrap that with `reqwest` so a Memex snapshot is just one file. The user
can move it between machines, e-mail it, archive it, etc. Privacy: the
snapshot includes embeddings + payload metadata, not the raw JSONL.

**Code pointer.** `src-tauri/src/indexer.rs::snapshot_export` /
`snapshot_import`.

---

## What's *not* shipped here (and why)

| Plan item | Status in v3 | Notes |
|---|---|---|
| BM42 sparse on `path` | superseded by `path_sparse` + `tool_sparse` with IDF modifier | Qdrant 1.18 SparseVectorParams + `Modifier::Idf` does what BM42 promised, native. |
| ColBERT late-interaction | wired (`content_late` slot exists, indexed) — **off** by default until T3.3 flips weight to ~0.25 | See [wired-but-dormant.md](./wired-but-dormant.md). |
| Custom analyzer for camelCase / snake_case | under investigation (T3.4 in qdrant-improvement-goal.md) | If 1.18 SDK doesn't expose the tokenizer knob, documented as a 1.18 limit. |
| Multi-collection per topic | future work | A single `memex_sessions_v3` collection has been enough for hackathon scale; revisit at 100k+ sessions. |
| Native file picker for snapshot | `window.prompt()` | `tauri-plugin-dialog` is a 5-line add; queued for polish. |

Each of these has a clean upgrade path; none of them are load-bearing for
the v3 demo story.

---

## Related docs

- [`wired-but-dormant.md`](./wired-but-dormant.md) — honest status flags per feature (on / off / not wired).
- [`benchmarks.md`](./benchmarks.md) — recall@10 / latency-p95 / index-size numbers.
- [`architecture.md`](./architecture.md) — sequence diagrams: index path, query path, snapshot lifecycle.
- The landing's `#qdrant` section (`index.html`) is the canonical UI version of this doc — every claim here is grep-able against `src-tauri/**` and is animated as a Q-card.
