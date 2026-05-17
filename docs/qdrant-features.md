# Memex × Qdrant — the five features

This is the engineer's tour of the five Qdrant features Memex is built on, and
why a vanilla "one vector per session" search wouldn't get you here.

Each section says:
1. **What you see** — the UI affordance.
2. **What Qdrant does** — the primitive being exercised.
3. **Why it matters** — what a single-vector competitor can't replicate.
4. **Code pointer** — where in this repo.

---

## 1. Lens slider — multi-named-vector scoring with weights

**What you see.** Five sliders in the sidebar — `content`, `tool`, `path`,
`error`, `code`. Drag any slider toward 0 to drop that lens; drag toward 2 to
bias toward it. Every result card shows the per-vector contribution as chips
so you can see *which lens earned this hit*.

**What Qdrant does.** Each session is one point with five named dense
vectors, all 384-d cosine BGE-small:

```protobuf
message VectorsConfig {
  oneof config {
    VectorParamsMap params_map = 2;   // ← we use this
  }
}
```

For each query we call `client.query()` once per non-zero weight, each time
selecting a single named vector with `.using("<name>")`. Then in Rust we
combine the per-vector scores as
`sum(score_i × weight_i / sum_of_weights)`.

**Why it matters.** With one vector per session you can't say "weight tool
calls 2× more than prose for this query". You could shoehorn it with RRF, but
RRF is rank-based and ignores weights. Qdrant's *named vectors per point*
gives us five orthogonal lenses on the same conversation, all stored in one
binary-quantized HNSW per name.

**Code pointer.** `src-tauri/src/indexer.rs::lens_search`.

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

**Code pointer.** `src-tauri/src/indexer.rs::mix_match`.

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
    SearchMatrixPointsBuilder::new(COLLECTION)
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

**What you see.** While you're working in another Claude Code session, Memex
polls `~/.claude/projects` for fresh `tool_result.is_error` events. When it
sees one, it asks Qdrant: *"have I solved a similar error before?"* If yes,
a banner slides into the bottom-right with the past-fix candidate. Click
"Open replay" and you land directly inside the relevant past session.

**What Qdrant does.**

```rust
client.query(
    QueryPointsBuilder::new(COLLECTION)
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

**Why it matters.** "Have I seen this before?" is the killer feature for an
engineer's session history. Without (a) a dedicated error lens and (b) a
cheap "had errors" filter, the recall feed gets noisy fast.

**Code pointer.** `src-tauri/src/indexer.rs::recall` +
`src-tauri/src/commands.rs::tail_recent_errors` + `src/main.js::pollRecall`.

---

## Snapshot — boring but essential

Qdrant has built-in snapshot export/import via HTTP:

```
POST /collections/{COLLECTION}/snapshots          → create + filename
GET  /collections/{COLLECTION}/snapshots/{name}   → download bytes
POST /collections/{COLLECTION}/snapshots/upload?priority=snapshot → restore
```

We wrap that with `reqwest` so a Memex snapshot is just one file. The user
can move it between machines, e-mail it, archive it, etc. Privacy: the
snapshot includes embeddings + payload metadata, not the raw JSONL.

**Code pointer.** `src-tauri/src/indexer.rs::snapshot_export` /
`snapshot_import`.

---

## What's *not* shipped here (and why)

| Plan item | Status | Reason |
|---|---|---|
| BM42 sparse on `path` (T2.2) | dense BGE-small for now | `fastembed-rs` 5.13.4 doesn't expose BM42; we run all five vectors dense for MVP. |
| Jina ColBERT v2 multi-vector (T2.6 / T3.4) | deferred | Same upstream gap. Fallback via `ort` crate is on the future-work list. |
| Native file picker for snapshot (T4.7) | `window.prompt()` | `tauri-plugin-dialog` is a 5-line add; queued for Phase 7 polish. |
| `notify` file watcher (T6.1) | polling | Polling is simpler, no permissions, no fd leaks. `notify` is in `Cargo.toml` for the swap. |

Each of these has a clean upgrade path; none of them are load-bearing for the
five-feature demo story.
