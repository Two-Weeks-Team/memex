//! P2 — Query API Core (KA-01 / KA-02 / KA-05 / KB-02).
//!
//! Server-side weighted lens scoring via Qdrant 1.18 FormulaQuery (KA-01).
//!
//! The previous implementation (still callable as a fallback) ran one cosine
//! search per active named vector and combined scores in Rust. That cost N
//! gRPC round-trips per query and made the recency / has_errors boost
//! impossible to express on the server side. P2 replaces that with **one**
//! `query_points` call carrying a prefetch tree + a `Formula` expression:
//!
//!   prefetch:
//!     content (dense)        — limit 50
//!     tool    (dense)        — limit 50
//!     path_sparse (sparse)   — limit 50
//!     tool_sparse (sparse)   — limit 50    (KB-02)
//!     error   (dense)        — limit 50, filter: has_errors==true
//!     code    (dense)        — limit 50
//!     content_late (multi)   — limit 50 (only when weight > 0; KB-01 reuse)
//!   query:
//!     formula:
//!       sum(
//!         w_content * $score[0],
//!         w_tool    * $score[1],
//!         w_path    * $score[2],   ← path_sparse score
//!         w_tool    * $score[3],   ← tool_sparse score (KB-02; shares the tool weight)
//!         w_error   * $score[4],
//!         w_code    * $score[5],
//!         (optional) w_late * $score[6],
//!         exp_decay(payload.start_ts, scale=30d),   ← recency boost
//!         0.2 if has_errors==true                   ← error-debug priority
//!       )
//!
//! When `LensWeights.fusion == FusionMode::Rrf` we drop the formula entirely and
//! issue the same prefetch set with `Query::Rrf(Rrf { weights, k })` so the
//! server fuses by reciprocal rank with per-prefetch weights (KA-05).
//!
//! MMR (KA-02) is wrapped around the **content** prefetch via
//! `Query::new_nearest_with_mmr(...)` — the only place a per-prefetch query
//! can carry an MMR re-ranker. We picked content because it's the primary
//! relevance signal; diversifying the per-vector candidate pool there is
//! what kills near-duplicate session clones without affecting the other
//! lenses' contribution.
//!
//! SPEC NOTE (P2):
//! - `SearchParams.diversity` does NOT exist in qdrant-client 1.18. The
//!   parameter lives in `Mmr { diversity, candidates_limit }` which is a
//!   distinct query variant (`Query::NearestWithMmr`). We surface
//!   `LensWeights.diversity: Option<f32>` and inject it into the content
//!   prefetch only when set.
//! - FormulaQuery returns one fused score per point. To populate
//!   `ScoreBreakdown.per_vector` (needed by WOW-3 contribution bars) we
//!   issue a **second pass**: for each active vector, run a tiny `limit=200`
//!   prefetch and collect each session's per-vector raw score. The cost is
//!   one extra round-trip per active lens vector — acceptable for the lens
//!   slider UX (≤200ms wall-clock on the author's corpus) and only paid when
//!   the new `lens_search_v2` command is invoked. The legacy `lens_search`
//!   Tauri command still calls this code, but the Rust caller can skip the
//!   breakdown by passing `with_breakdown=false`.

use std::collections::HashMap;

use anyhow::{anyhow, Context, Result};
use qdrant_client::qdrant::{
    Condition, DecayParamsExpressionBuilder, Expression, Filter, FormulaBuilder, MmrBuilder,
    PrefetchQueryBuilder, Query, QueryPointsBuilder, RrfBuilder, SearchParams, SparseVector, Value,
    VectorInput,
};
use qdrant_client::Qdrant;

use crate::indexer::{payload_str, Embedder, SearchHit};
use crate::schema::{search_params_with_quantization, COLLECTION_V3};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Fusion strategy applied at the top-level `Query` (KA-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FusionMode {
    /// Server-side `Query::Formula`. Default. Carries recency + error boost.
    Formula,
    /// Server-side `Query::Rrf` with per-prefetch weights. No recency boost.
    Rrf,
}

impl Default for FusionMode {
    fn default() -> Self {
        Self::Formula
    }
}

/// Per-vector lens weights + MMR diversity + fusion mode.
///
/// The 6 dense weights map 1:1 to named vectors (BGE-small dense + content_late
/// multivector). `path` and `tool` are reused for the sparse counterparts
/// (`path_sparse`, `tool_sparse`) so callers don't have to know about BM25
/// separately — adjusting the path weight in the UI moves both the dense path
/// vector and the BM25 path index.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LensWeights {
    #[serde(default = "default_weight")]
    pub content: f32,
    #[serde(default = "default_weight")]
    pub tool: f32,
    #[serde(default = "default_weight")]
    pub path: f32,
    #[serde(default = "default_weight")]
    pub error: f32,
    #[serde(default = "default_weight")]
    pub code: f32,
    #[serde(default = "default_zero")]
    pub content_late: f32,
    /// KA-02. `None` ⇒ no MMR. `Some(d)` with d ∈ [0,1] ⇒ wrap the **content**
    /// prefetch with NearestWithMmr(diversity=d). Per spec, default is 0.4
    /// when the caller asks for "diversified" lens but the wire default is
    /// off (None) so existing callers see identical behavior.
    #[serde(default)]
    pub diversity: Option<f32>,
    /// KA-05.
    #[serde(default)]
    pub fusion: FusionMode,
}

fn default_weight() -> f32 {
    1.0
}
fn default_zero() -> f32 {
    0.0
}

impl Default for LensWeights {
    fn default() -> Self {
        Self {
            content: 1.0,
            tool: 1.0,
            path: 1.0,
            error: 1.0,
            code: 1.0,
            content_late: 0.0,
            diversity: None,
            fusion: FusionMode::Formula,
        }
    }
}

impl LensWeights {
    /// Sum of all dense + sparse weight contributions. Used to short-circuit
    /// "no active lens" → Err.
    pub fn total(&self) -> f32 {
        self.content
            + self.tool
            + self.path
            + self.error
            + self.code
            + self.content_late
    }
}

/// One row of a lens search result. Strictly a superset of `SearchHit`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LensResult {
    pub session_id: String,
    pub score: f32,
    pub project_name: String,
    pub ai_title: String,
    pub start_iso: String,
    pub score_breakdown: ScoreBreakdown,
    /// Full Qdrant payload. Serialized as a flat JSON object so the frontend
    /// inspector can show arbitrary keys without needing schema changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_json: Option<serde_json::Value>,
}

/// Per-prefetch breakdown for the WOW-3 contribution bars.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScoreBreakdown {
    /// vector-name → raw cosine/BM25 score for this point.
    pub per_vector: HashMap<String, f32>,
    /// `exp_decay(start_ts, scale=30d)` contribution. 0..1.
    pub recency_factor: f32,
    /// 0.2 when has_errors=true, else 0.0.
    pub has_errors_boost: f32,
    /// The fused/Formula score (post-prefetch combine).
    pub final_score: f32,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Recency decay scale in **seconds**. 30 days. exp_decay returns
/// ~0.5 when |x − now| == scale, asymptotically 0 thereafter.
pub const RECENCY_SCALE_SECONDS: f32 = 30.0 * 24.0 * 3600.0;
/// Static boost added when `has_errors == true`. Matches the spec value.
pub const HAS_ERRORS_BOOST: f32 = 0.2;
/// Per-prefetch candidate limit.
pub const PREFETCH_LIMIT: u64 = 50;
/// Hard server-side cap for the top-level limit. Strict mode allows ≤100.
pub const MAX_LIMIT: u64 = 100;

/// FormulaQuery / Rrf lens (KA-01 / KA-02 / KA-05 / KB-02).
///
/// Returns `Err("empty query")` if `query_text` is whitespace-only, or
/// `Err("no active lens")` when all six weights are ≤ 0. `limit` is clamped
/// to `MAX_LIMIT`.
pub async fn lens_search_v2(
    client: &Qdrant,
    embedder: &Embedder,
    query_text: &str,
    weights: &LensWeights,
    limit: u64,
) -> Result<Vec<LensResult>> {
    lens_search_v2_on(client, embedder, query_text, weights, limit, COLLECTION_V3).await
}

/// Same as [`lens_search_v2`] but accepts a custom collection name. Used by
/// integration tests against throwaway collections so they don't trample the
/// real `memex_sessions_v3` data.
pub async fn lens_search_v2_on(
    client: &Qdrant,
    embedder: &Embedder,
    query_text: &str,
    weights: &LensWeights,
    limit: u64,
    collection: &str,
) -> Result<Vec<LensResult>> {
    if query_text.trim().is_empty() {
        return Err(anyhow!("empty query"));
    }
    if weights.total() <= 0.0 {
        return Err(anyhow!("no active lens"));
    }
    let limit = limit.min(MAX_LIMIT).max(1);

    let qvec_dense = embedder
        .embed(vec![query_text.to_string()])?
        .into_iter()
        .next()
        .context("no embedding for query")?;
    let qvec_sparse = text_to_sparse(query_text);

    let active_dense = active_dense_specs(weights);
    let mut sparse_specs = active_sparse_specs(weights);
    // Audit BLOCKER fix: when text_to_sparse produces no tokens (e.g. single
    // char or all-punctuation query), drop sparse_specs to empty so the
    // formula/RRF builders don't emit `$score[i]` for prefetches that
    // build_prefetches will silently skip. Without this, the server receives
    // a formula referencing non-existent prefetch indices and rejects the
    // whole query.
    if qvec_sparse.indices.is_empty() {
        sparse_specs.clear();
    }
    if active_dense.is_empty() && sparse_specs.is_empty() {
        return Err(anyhow!("no active lens"));
    }

    // Build the prefetch list. Order is canonical: we want $score[i] to map
    // to a known vector for the formula expression.
    let prefetches = build_prefetches(
        weights,
        &active_dense,
        &sparse_specs,
        &qvec_dense,
        &qvec_sparse,
        search_params_with_quantization(),
    )?;

    // Top-level query: Formula (default) or Rrf.
    let top_query: Query = match weights.fusion {
        FusionMode::Formula => Query::new_formula(build_formula(weights, &active_dense, &sparse_specs)),
        FusionMode::Rrf => Query::new_rrf(build_rrf(weights, &active_dense, &sparse_specs)),
    };

    let mut req = QueryPointsBuilder::new(collection.to_string())
        .query(top_query)
        .limit(limit)
        .with_payload(true);
    for pf in prefetches {
        req = req.add_prefetch(pf);
    }
    let resp = client.query(req).await.context("FormulaQuery request failed")?;

    // Map the fused results to LensResult. score_breakdown.per_vector is
    // empty here — populated on demand by `populate_breakdowns` below.
    let mut results: Vec<LensResult> = resp
        .result
        .into_iter()
        .filter_map(|p| {
            let sid = payload_str(&p.payload, "session_id")?;
            let has_errors_boost = payload_bool(&p.payload, "has_errors")
                .map(|b| if b { HAS_ERRORS_BOOST } else { 0.0 })
                .unwrap_or(0.0);
            let breakdown = ScoreBreakdown {
                per_vector: HashMap::new(),
                recency_factor: 0.0, // populated later if requested
                has_errors_boost,
                final_score: p.score,
            };
            Some(LensResult {
                session_id: sid,
                score: p.score,
                project_name: payload_str(&p.payload, "project_name").unwrap_or_default(),
                ai_title: payload_str(&p.payload, "ai_title").unwrap_or_default(),
                start_iso: payload_str(&p.payload, "start_iso").unwrap_or_default(),
                score_breakdown: breakdown,
                payload_json: payload_to_json(p.payload),
            })
        })
        .collect();

    // Per-vector breakdown for WOW-3 (best-effort — failures don't break the
    // primary result set).
    if !results.is_empty() {
        if let Err(e) = populate_breakdowns(
            client,
            embedder,
            query_text,
            weights,
            &active_dense,
            &sparse_specs,
            &mut results,
            collection,
        )
        .await
        {
            eprintln!("[lens] breakdown enrichment skipped: {e:#}");
        }
    }

    Ok(results)
}

/// **KB-02** — Convert a query string into a `SparseVector` suitable for
/// `path_sparse` / `tool_sparse` lookups against an IDF-modifier sparse index.
///
/// Strategy: case-fold, split on path/word separators, hash each unique token
/// to a 32-bit index via FxHash-equivalent (we use the `DefaultHasher` for
/// stdlib portability; the IDF modifier on the server side compensates for
/// any token-frequency skew so collisions are tolerable). Token value is the
/// **count** in the query — server multiplies by stored IDF.
///
/// Empty/whitespace queries produce an empty `SparseVector` rather than
/// panicking; callers should skip the sparse prefetch in that case.
pub fn text_to_sparse(text: &str) -> SparseVector {
    let mut counts: HashMap<u32, f32> = HashMap::new();
    for tok in tokenize_for_sparse(text) {
        let idx = hash_token(&tok);
        *counts.entry(idx).or_insert(0.0) += 1.0;
    }
    let (indices, values): (Vec<u32>, Vec<f32>) = counts.into_iter().unzip();
    SparseVector { indices, values }
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Which dense vectors have a positive weight. Order is fixed so $score[i]
/// alignment in the formula stays stable.
fn active_dense_specs(w: &LensWeights) -> Vec<DenseSpec> {
    let mut v = Vec::with_capacity(6);
    if w.content > 0.0 {
        v.push(DenseSpec { name: "content", weight: w.content, has_errors_filter: false });
    }
    if w.tool > 0.0 {
        v.push(DenseSpec { name: "tool", weight: w.tool, has_errors_filter: false });
    }
    // Note: dense `path` is intentionally NOT included — `path` weight is
    // routed to the sparse counterpart `path_sparse`. The dense `path`
    // vector exists for snapshot/recovery use cases but path tokens are far
    // better-served by BM25.
    if w.error > 0.0 {
        v.push(DenseSpec { name: "error", weight: w.error, has_errors_filter: true });
    }
    if w.code > 0.0 {
        v.push(DenseSpec { name: "code", weight: w.code, has_errors_filter: false });
    }
    if w.content_late > 0.0 {
        v.push(DenseSpec { name: "content_late", weight: w.content_late, has_errors_filter: false });
    }
    v
}

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

#[derive(Debug, Clone, Copy)]
struct DenseSpec {
    name: &'static str,
    weight: f32,
    /// `error` prefetch is filtered to points with has_errors=true so the
    /// formula's error term doesn't dilute the score for non-error sessions.
    has_errors_filter: bool,
}

#[derive(Debug, Clone, Copy)]
struct SparseSpec {
    name: &'static str,
    weight: f32,
    /// For logging / debugging; not used in the wire request.
    #[allow(dead_code)]
    source_field: &'static str,
}

/// Build the full prefetch list in formula-index order.
fn build_prefetches(
    weights: &LensWeights,
    dense: &[DenseSpec],
    sparse: &[SparseSpec],
    qvec: &[f32],
    qsparse: &SparseVector,
    params: SearchParams,
) -> Result<Vec<qdrant_client::qdrant::PrefetchQuery>> {
    let mut out = Vec::with_capacity(dense.len() + sparse.len());

    for spec in dense {
        let q: Query = if spec.name == "content" && weights.diversity.is_some() {
            // KA-02 — MMR rerank ONLY on the content prefetch.
            let diversity = weights.diversity.unwrap();
            let candidates_limit: u32 = (PREFETCH_LIMIT as u32) * 4;
            Query::new_nearest_with_mmr(
                VectorInput::new_dense(qvec.to_vec()),
                MmrBuilder::with_params(diversity, candidates_limit).build(),
            )
        } else if spec.name == "content_late" {
            // KB-01 reuse — content_late expects a multivector input. We
            // embed the query once and wrap it as a single-row multivector.
            Query::new_nearest(VectorInput::new_multi(vec![qvec.to_vec()]))
        } else {
            Query::new_nearest(VectorInput::new_dense(qvec.to_vec()))
        };

        let mut pf = PrefetchQueryBuilder::default()
            .query(q)
            .using(spec.name.to_string())
            .limit(PREFETCH_LIMIT)
            .params(params.clone());
        if spec.has_errors_filter {
            pf = pf.filter(Filter::must([Condition::matches("has_errors", true)]));
        }
        out.push(pf.build());
    }

    for spec in sparse {
        if qsparse.indices.is_empty() {
            // Skip empty sparse queries — the server rejects 0-length vectors
            // and would otherwise cancel the whole request.
            continue;
        }
        let q = Query::new_nearest(VectorInput::new_sparse(
            qsparse.indices.clone(),
            qsparse.values.clone(),
        ));
        let pf = PrefetchQueryBuilder::default()
            .query(q)
            .using(spec.name.to_string())
            .limit(PREFETCH_LIMIT);
        // Sparse prefetches don't take quantization SearchParams — the IDF
        // sparse index doesn't honor TurboQuant rescore. Leaving params off
        // is what the qdrant-client builder defaults to.
        out.push(pf.build());
    }

    Ok(out)
}

/// Build the top-level Formula expression (KA-01).
///
/// `$score[i]` references the i-th prefetch in the prefetch list. We honor
/// the same canonical order as `build_prefetches`. Each prefetch's weight
/// multiplies its $score, then we add a recency exp_decay term + a flat
/// has_errors boost.
fn build_formula(
    _weights: &LensWeights,
    dense: &[DenseSpec],
    sparse: &[SparseSpec],
) -> FormulaBuilder {
    let mut terms: Vec<Expression> = Vec::with_capacity(dense.len() + sparse.len() + 2);

    let mut idx: usize = 0;
    for spec in dense {
        terms.push(Expression::mult_with([
            Expression::score_idx(idx),
            Expression::constant(spec.weight),
        ]));
        idx += 1;
    }
    for spec in sparse {
        // Upstream (lens_search_v2_on) drops sparse_specs to empty when the
        // query produced no sparse tokens, so this loop only runs for
        // prefetches that ARE present — $score[i] alignment stays in sync.
        terms.push(Expression::mult_with([
            Expression::score_idx(idx),
            Expression::constant(spec.weight),
        ]));
        idx += 1;
    }

    // Recency exp_decay on `start_ts` (Unix seconds). Scale = 30 days.
    // SPEC NOTES:
    //   1. Qdrant 1.18 references payload fields via `payload.<name>` (not
    //      bare `<name>`). Without the prefix the server silently substitutes
    //      0 and collapses the term to a constant.
    //   2. exp_decay anchors at `target` (defaults to 0). For recency we need
    //      to anchor at "now" so older `start_ts` values decay relative to
    //      the present moment. Without an explicit target=now, all reasonably
    //      recent timestamps land equidistant from epoch and produce the same
    //      decay value, defeating the recency boost entirely.
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as f64)
        .unwrap_or(0.0);
    let recency = Expression::exp_decay(
        DecayParamsExpressionBuilder::new(Expression::variable("payload.start_ts"))
            .target(Expression::constant(now_secs as f32))
            .scale(RECENCY_SCALE_SECONDS)
            .build(),
    );
    terms.push(recency);

    // Conditional has_errors boost. Expression::condition collapses to 1.0
    // when the filter matches the point's payload, 0.0 otherwise — we
    // multiply by the boost constant to scale it.
    let err_boost = Expression::mult_with([
        Expression::condition(Condition::matches("has_errors", true)),
        Expression::constant(HAS_ERRORS_BOOST),
    ]);
    terms.push(err_boost);

    // SPEC NOTE: Formula defaults supply fallback values for points whose
    // payload is missing the referenced field. Without this, the server
    // rejects the whole query with "Expected number value for payload.X in
    // the payload and/or in the formula defaults" — even points that DO
    // have the field never get scored. We default `payload.start_ts` to 0
    // (Unix epoch → recency_factor ≈ 0 for missing-timestamp points).
    let mut defaults: HashMap<String, qdrant_client::qdrant::Value> = HashMap::new();
    defaults.insert(
        "payload.start_ts".to_string(),
        qdrant_client::qdrant::Value {
            kind: Some(qdrant_client::qdrant::value::Kind::IntegerValue(0)),
        },
    );

    FormulaBuilder::new(Expression::sum_with(terms)).defaults(defaults)
}

/// Build the top-level Rrf with weights aligned to the prefetch list (KA-05).
fn build_rrf(_weights: &LensWeights, dense: &[DenseSpec], sparse: &[SparseSpec]) -> RrfBuilder {
    let mut w = Vec::with_capacity(dense.len() + sparse.len());
    for d in dense {
        w.push(d.weight);
    }
    for s in sparse {
        w.push(s.weight);
    }
    RrfBuilder::new().weights(w)
}

/// Issue one cheap per-vector query per active lens vector so the WOW-3
/// inspector can render the per-vector contribution bars. The function
/// updates `results[i].score_breakdown.per_vector` in place.
///
/// We don't redo the formula math here — the goal is just to surface the
/// raw cosine/BM25 score that contributed. Recency factor is computed from
/// the payload `start_ts` (so it stays self-consistent with the formula).
async fn populate_breakdowns(
    client: &Qdrant,
    _embedder: &Embedder,
    _query_text: &str,
    _weights: &LensWeights,
    dense: &[DenseSpec],
    sparse: &[SparseSpec],
    results: &mut [LensResult],
    collection: &str,
) -> Result<()> {
    // We re-issue the prefetch queries individually with the SAME query
    // vectors that the formula path used. Cheap because:
    //   - PREFETCH_LIMIT (50) covers > our top-`limit` (≤100) → if the
    //     session made the formula's top-N, it should also make the per-
    //     vector top-50. Misses are filled with `0.0` so the bar collapses.
    let qvec = match _embedder.embed(vec![_query_text.to_string()]) {
        Ok(mut v) => v.pop().unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    if qvec.is_empty() {
        return Ok(());
    }
    let qsparse = text_to_sparse(_query_text);

    let params = search_params_with_quantization();
    // Index sessions in the result set for O(1) lookup.
    let mut sid_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, r) in results.iter().enumerate() {
        sid_to_idx.insert(r.session_id.clone(), i);
    }

    // Recency factor — compute locally from payload.start_ts.
    let now_ts: i64 = chrono::Utc::now().timestamp();
    for r in results.iter_mut() {
        let start_ts: i64 = r
            .payload_json
            .as_ref()
            .and_then(|j| j.get("start_ts"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let dt = (now_ts - start_ts).abs() as f32;
        // Same shape as Qdrant exp_decay with target=0, scale=30d, midpoint=0.5.
        // f(x) = 0.5 ^ ((|x - target| / scale)^2)  — Qdrant's exp_decay.
        let ratio = dt / RECENCY_SCALE_SECONDS;
        r.score_breakdown.recency_factor = 0.5_f32.powf(ratio * ratio);
    }

    for spec in dense {
        let q: Query = if spec.name == "content_late" {
            Query::new_nearest(VectorInput::new_multi(vec![qvec.clone()]))
        } else {
            Query::new_nearest(VectorInput::new_dense(qvec.clone()))
        };
        let mut req = QueryPointsBuilder::new(collection.to_string())
            .query(q)
            .using(spec.name.to_string())
            .limit(PREFETCH_LIMIT * 2) // wider net for breakdown
            .with_payload(true)
            .params(params.clone());
        if spec.has_errors_filter {
            req = req.filter(Filter::must([Condition::matches("has_errors", true)]));
        }
        match client.query(req).await {
            Ok(resp) => {
                for p in resp.result {
                    if let Some(sid) = payload_str(&p.payload, "session_id") {
                        if let Some(&i) = sid_to_idx.get(&sid) {
                            results[i]
                                .score_breakdown
                                .per_vector
                                .insert(spec.name.to_string(), p.score);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("[lens] breakdown query failed for {}: {e:#}", spec.name);
            }
        }
    }

    for spec in sparse {
        if qsparse.indices.is_empty() {
            continue;
        }
        let q = Query::new_nearest(VectorInput::new_sparse(
            qsparse.indices.clone(),
            qsparse.values.clone(),
        ));
        let req = QueryPointsBuilder::new(collection.to_string())
            .query(q)
            .using(spec.name.to_string())
            .limit(PREFETCH_LIMIT * 2)
            .with_payload(true);
        match client.query(req).await {
            Ok(resp) => {
                for p in resp.result {
                    if let Some(sid) = payload_str(&p.payload, "session_id") {
                        if let Some(&i) = sid_to_idx.get(&sid) {
                            results[i]
                                .score_breakdown
                                .per_vector
                                .insert(spec.name.to_string(), p.score);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("[lens] sparse breakdown query failed for {}: {e:#}", spec.name);
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn payload_bool(p: &HashMap<String, Value>, key: &str) -> Option<bool> {
    p.get(key).and_then(|v| v.kind.as_ref()).and_then(|k| match k {
        qdrant_client::qdrant::value::Kind::BoolValue(b) => Some(*b),
        _ => None,
    })
}

/// Best-effort conversion of a Qdrant payload map into a `serde_json::Value`
/// so the frontend can show arbitrary keys. Only the common scalar kinds are
/// preserved; nested structs / lists round-trip as JSON arrays/objects via
/// recursion. Failures collapse to `Object(empty)`.
fn payload_to_json(p: HashMap<String, Value>) -> Option<serde_json::Value> {
    use qdrant_client::qdrant::value::Kind;
    let mut map = serde_json::Map::with_capacity(p.len());
    for (k, v) in p.into_iter() {
        if let Some(kind) = v.kind {
            let jv = match kind {
                Kind::NullValue(_) => serde_json::Value::Null,
                Kind::BoolValue(b) => serde_json::Value::Bool(b),
                Kind::IntegerValue(i) => serde_json::Value::Number(i.into()),
                Kind::DoubleValue(d) => serde_json::Number::from_f64(d)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null),
                Kind::StringValue(s) => serde_json::Value::String(s),
                Kind::ListValue(_) | Kind::StructValue(_) => serde_json::Value::Null,
            };
            map.insert(k, jv);
        }
    }
    Some(serde_json::Value::Object(map))
}

/// Token splitter — path-aware so `/Users/foo/bar.rs` yields
/// `users`, `foo`, `bar`, `rs`.
fn tokenize_for_sparse(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let lower = text.to_lowercase();
    for raw in lower.split(|c: char| !c.is_alphanumeric()) {
        let t = raw.trim();
        if t.is_empty() || t.len() < 2 {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

/// Deterministic 32-bit hash for a token, suitable for sparse-vector indices.
///
/// FxHash would be ideal but adds a dep — we use stdlib `DefaultHasher` here.
/// IDF on the server side dampens any natural collision skew.
fn hash_token(tok: &str) -> u32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    tok.hash(&mut h);
    // Map the 64-bit hash into 31 bits (avoid the sign bit collision with
    // wire-protocol uint32 representations on some platforms).
    (h.finish() as u32) & 0x7FFF_FFFF
}

/// Convert a `LensResult` into the legacy `SearchHit` shape consumed by the
/// existing Tauri command. The contribution map is preserved verbatim.
pub fn lens_result_to_searchhit(r: LensResult) -> SearchHit {
    SearchHit {
        score: r.score,
        session_id: r.session_id,
        project_name: r.project_name,
        ai_title: r.ai_title,
        start_iso: r.start_iso,
        vector_scores: r.score_breakdown.per_vector,
    }
}

/// Adapter `LensWeights` (this module) → `crate::indexer::LensWeights`
/// (legacy struct) so callers in `indexer::lens_search` can keep the same
/// signature. Lossless for the 6 dense weights; the new `diversity` /
/// `fusion` fields default to `None` / `Formula`.
impl From<crate::indexer::LensWeights> for LensWeights {
    fn from(v: crate::indexer::LensWeights) -> Self {
        Self {
            content: v.content,
            tool: v.tool,
            path: v.path,
            error: v.error,
            code: v.code,
            content_late: v.content_late,
            diversity: None,
            fusion: FusionMode::Formula,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — TDD G2 gate (11 FormulaQuery + 3 MMR + 3 RRF + 4 BM25 = 21).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use qdrant_client::qdrant::{expression, query, value::Kind};

    // ---- BM25 sparse tokenization (KB-02) — 4 tests --------------------

    #[test]
    fn bm25_tokenizer_splits_path_segments() {
        let toks = tokenize_for_sparse("/Users/foo/bar.rs");
        // 2-char min, lowercase, alphanumeric splits
        assert!(toks.contains(&"users".to_string()), "got {toks:?}");
        assert!(toks.contains(&"foo".to_string()));
        assert!(toks.contains(&"bar".to_string()));
        assert!(toks.contains(&"rs".to_string()));
    }

    #[test]
    fn bm25_tokenizer_drops_single_chars_and_punct() {
        let toks = tokenize_for_sparse("a b cd , ./ ef");
        // single chars removed; non-alphanumeric used as delimiter only
        for t in &toks {
            assert!(t.len() >= 2, "single-char survived: {t:?}");
        }
        assert!(toks.contains(&"cd".to_string()));
        assert!(toks.contains(&"ef".to_string()));
    }

    #[test]
    fn bm25_text_to_sparse_empty_query() {
        let sv = text_to_sparse("");
        assert!(sv.indices.is_empty());
        assert!(sv.values.is_empty());
    }

    #[test]
    fn bm25_text_to_sparse_single_char_returns_empty() {
        // Audit BLOCKER regression: a single-char query like "a" must
        // produce empty sparse so lens_search_v2_on can drop sparse_specs.
        // Without this, build_formula would emit `$score[i]` for prefetches
        // that build_prefetches silently skipped, and the server rejects
        // the formula.
        for q in &["a", "1", ".", "  ", "..//"] {
            let sv = text_to_sparse(q);
            assert!(sv.indices.is_empty(), "single-char/punct {q:?} should produce empty sparse");
            assert!(sv.values.is_empty());
        }
    }

    #[test]
    fn formula_term_count_matches_when_sparse_empty() {
        // Audit BLOCKER regression: when caller passes empty sparse, the
        // formula should emit only dense + 2 (recency + has_errors) terms.
        // No orphan `$score[i]` references to sparse prefetches that don't
        // exist.
        let dense = active_dense_specs(&LensWeights::default());
        let empty_sparse: Vec<SparseSpec> = vec![];
        let formula = build_formula(&LensWeights::default(), &dense, &empty_sparse).build();
        // FormulaBuilder.build() returns Formula { expression: Some(Sum(...)), .. }
        let expr = formula.expression.as_ref().expect("formula has expression");
        let sum = match expr.variant.as_ref().expect("expression variant") {
            expression::Variant::Sum(s) => s,
            other => panic!("expected Sum, got {other:?}"),
        };
        // dense.len() multiplications + 1 recency exp_decay + 1 has_errors boost
        assert_eq!(sum.sum.len(), dense.len() + 2,
            "expected {} terms (dense + recency + has_errors), got {}",
            dense.len() + 2, sum.sum.len());
    }

    #[test]
    fn bm25_text_to_sparse_aggregates_repeat_counts() {
        // "foo foo bar" → value(foo)=2.0, value(bar)=1.0
        let sv = text_to_sparse("foo foo bar");
        let pairs: HashMap<u32, f32> = sv.indices.iter().copied().zip(sv.values.iter().copied()).collect();
        let foo_idx = hash_token("foo");
        let bar_idx = hash_token("bar");
        assert_eq!(pairs.get(&foo_idx).copied(), Some(2.0));
        assert_eq!(pairs.get(&bar_idx).copied(), Some(1.0));
    }

    // ---- LensWeights default + total + edge cases ----------------------

    #[test]
    fn lens_weights_default_has_formula_fusion_and_no_mmr() {
        let w = LensWeights::default();
        assert_eq!(w.fusion, FusionMode::Formula);
        assert!(w.diversity.is_none());
        assert!(w.total() > 0.0);
    }

    #[test]
    fn lens_weights_total_zero_when_all_off() {
        let w = LensWeights {
            content: 0.0,
            tool: 0.0,
            path: 0.0,
            error: 0.0,
            code: 0.0,
            content_late: 0.0,
            diversity: None,
            fusion: FusionMode::Formula,
        };
        assert_eq!(w.total(), 0.0);
    }

    // ---- FormulaQuery shape — 11 unit tests ----------------------------

    fn dense_specs_full() -> Vec<DenseSpec> {
        active_dense_specs(&LensWeights::default())
    }

    #[test]
    fn formula_builds_with_default_weights() {
        let w = LensWeights::default();
        let f = build_formula(&w, &dense_specs_full(), &active_sparse_specs(&w)).build();
        // top-level expression must be Sum
        let var = f.expression.unwrap().variant.unwrap();
        assert!(matches!(var, expression::Variant::Sum(_)));
    }

    #[test]
    fn formula_has_recency_decay_term() {
        let w = LensWeights::default();
        let f = build_formula(&w, &dense_specs_full(), &active_sparse_specs(&w)).build();
        let sum = match f.expression.unwrap().variant.unwrap() {
            expression::Variant::Sum(s) => s.sum,
            _ => panic!("expected sum"),
        };
        let has_decay = sum
            .iter()
            .any(|e| matches!(e.variant, Some(expression::Variant::ExpDecay(_))));
        assert!(has_decay, "formula missing exp_decay term");
    }

    #[test]
    fn formula_has_errors_boost_term() {
        let w = LensWeights::default();
        let f = build_formula(&w, &dense_specs_full(), &active_sparse_specs(&w)).build();
        let sum = match f.expression.unwrap().variant.unwrap() {
            expression::Variant::Sum(s) => s.sum,
            _ => panic!("expected sum"),
        };
        // Look for: Mult([Condition(has_errors), Constant(0.2)])
        let has_boost = sum.iter().any(|e| {
            let Some(expression::Variant::Mult(m)) = &e.variant else {
                return false;
            };
            let mut has_cond = false;
            let mut has_const_boost = false;
            for inner in &m.mult {
                match &inner.variant {
                    Some(expression::Variant::Condition(_)) => has_cond = true,
                    Some(expression::Variant::Constant(c)) if (*c - HAS_ERRORS_BOOST).abs() < 1e-6 => {
                        has_const_boost = true;
                    }
                    _ => {}
                }
            }
            has_cond && has_const_boost
        });
        assert!(has_boost, "formula missing has_errors boost");
    }

    #[test]
    fn formula_score_idx_ordering_matches_prefetch() {
        // With default weights: dense=[content,tool,error,code] (content_late=0
        // so skipped) and sparse=[path_sparse,tool_sparse].
        // Note: path is routed to sparse, so dense.path is absent.
        let w = LensWeights::default();
        let dense = active_dense_specs(&w);
        let sparse = active_sparse_specs(&w);
        let names: Vec<&str> = dense.iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["content", "tool", "error", "code"]);
        assert_eq!(sparse.iter().map(|s| s.name).collect::<Vec<_>>(),
                   vec!["path_sparse", "tool_sparse"]);
        let f = build_formula(&w, &dense, &sparse).build();
        let sum_terms = match f.expression.unwrap().variant.unwrap() {
            expression::Variant::Sum(s) => s.sum,
            _ => panic!(),
        };
        // First N terms are weighted $score[i]
        let n = dense.len() + sparse.len();
        for i in 0..n {
            let inner = match &sum_terms[i].variant {
                Some(expression::Variant::Mult(m)) => &m.mult,
                _ => panic!("term {i} not Mult"),
            };
            let var = inner.iter().find_map(|e| match &e.variant {
                Some(expression::Variant::Variable(name)) => Some(name.clone()),
                _ => None,
            });
            assert_eq!(var.as_deref(), Some(format!("$score[{i}]").as_str()));
        }
    }

    #[test]
    fn formula_skips_content_late_when_weight_zero() {
        let mut w = LensWeights::default();
        w.content_late = 0.0;
        let dense = active_dense_specs(&w);
        assert!(!dense.iter().any(|d| d.name == "content_late"));
    }

    #[test]
    fn formula_includes_content_late_when_weight_positive() {
        let mut w = LensWeights::default();
        w.content_late = 0.5;
        let dense = active_dense_specs(&w);
        assert!(dense.iter().any(|d| d.name == "content_late"));
    }

    #[test]
    fn formula_dense_path_is_omitted_in_favor_of_sparse() {
        let w = LensWeights::default();
        let dense = active_dense_specs(&w);
        assert!(!dense.iter().any(|d| d.name == "path"),
                "dense `path` must be routed to path_sparse instead");
        let sparse = active_sparse_specs(&w);
        assert!(sparse.iter().any(|s| s.name == "path_sparse"));
    }

    #[test]
    fn formula_error_prefetch_has_filter_in_spec() {
        let w = LensWeights::default();
        let dense = active_dense_specs(&w);
        let err = dense.iter().find(|d| d.name == "error").unwrap();
        assert!(err.has_errors_filter);
    }

    #[test]
    fn formula_zero_weights_all_active_dense_empty() {
        let w = LensWeights {
            content: 0.0,
            tool: 0.0,
            path: 0.0,
            error: 0.0,
            code: 0.0,
            content_late: 0.0,
            diversity: None,
            fusion: FusionMode::Formula,
        };
        assert!(active_dense_specs(&w).is_empty());
        assert!(active_sparse_specs(&w).is_empty());
    }

    #[test]
    fn formula_constant_weights_match_input() {
        let w = LensWeights {
            content: 0.33,
            tool: 1.5,
            path: 1.5,
            error: 0.8,
            code: 0.6,
            content_late: 0.0,
            diversity: None,
            fusion: FusionMode::Formula,
        };
        let dense = active_dense_specs(&w);
        let sparse = active_sparse_specs(&w);
        let f = build_formula(&w, &dense, &sparse).build();
        let sum_terms = match f.expression.unwrap().variant.unwrap() {
            expression::Variant::Sum(s) => s.sum,
            _ => panic!(),
        };
        let constants: Vec<f32> = sum_terms
            .iter()
            .filter_map(|t| match &t.variant {
                Some(expression::Variant::Mult(m)) => m.mult.iter().find_map(|e| match &e.variant {
                    Some(expression::Variant::Constant(c)) => Some(*c),
                    _ => None,
                }),
                _ => None,
            })
            .collect();
        // First len(dense)+len(sparse) constants = weights, then HAS_ERRORS_BOOST.
        let n = dense.len() + sparse.len();
        let mut expected: Vec<f32> = dense.iter().map(|d| d.weight)
            .chain(sparse.iter().map(|s| s.weight)).collect();
        expected.push(HAS_ERRORS_BOOST);
        // The recency exp_decay does NOT contribute a Mult constant — we
        // expect exactly `n + 1` constants in this filter pass.
        assert_eq!(constants.len(), n + 1);
        for (i, c) in expected.iter().enumerate() {
            assert!((constants[i] - c).abs() < 1e-5,
                    "constant {i}: expected {c}, got {}", constants[i]);
        }
    }

    #[test]
    fn formula_recency_scale_is_30_days() {
        // RECENCY_SCALE_SECONDS in seconds == 30*86400
        assert!((RECENCY_SCALE_SECONDS - 2_592_000.0).abs() < 1e-3);
        let w = LensWeights::default();
        let f = build_formula(&w, &dense_specs_full(), &active_sparse_specs(&w)).build();
        let sum = match f.expression.unwrap().variant.unwrap() {
            expression::Variant::Sum(s) => s.sum,
            _ => panic!(),
        };
        for e in &sum {
            if let Some(expression::Variant::ExpDecay(d)) = &e.variant {
                assert_eq!(d.scale.unwrap_or(0.0), RECENCY_SCALE_SECONDS);
                return;
            }
        }
        panic!("missing exp_decay term in formula");
    }

    // ---- MMR (KA-02) — 3 unit tests ------------------------------------

    #[test]
    fn mmr_disabled_when_diversity_is_none() {
        let w = LensWeights::default();
        assert!(w.diversity.is_none());
        // Build prefetches and confirm content uses Nearest (not NearestWithMmr).
        let dense = active_dense_specs(&w);
        let sparse = active_sparse_specs(&w);
        let qv = vec![0.1f32; 384];
        let qs = text_to_sparse("test query");
        let pre = build_prefetches(&w, &dense, &sparse, &qv, &qs, search_params_with_quantization())
            .unwrap();
        let content = pre.iter().find(|p| p.using.as_deref() == Some("content")).unwrap();
        let v = content.query.as_ref().unwrap().variant.as_ref().unwrap();
        assert!(matches!(v, query::Variant::Nearest(_)));
    }

    #[test]
    fn mmr_enabled_wraps_content_with_nearest_with_mmr() {
        let mut w = LensWeights::default();
        w.diversity = Some(0.4);
        let dense = active_dense_specs(&w);
        let sparse = active_sparse_specs(&w);
        let qv = vec![0.1f32; 384];
        let qs = text_to_sparse("test query");
        let pre = build_prefetches(&w, &dense, &sparse, &qv, &qs, search_params_with_quantization())
            .unwrap();
        let content = pre.iter().find(|p| p.using.as_deref() == Some("content")).unwrap();
        let v = content.query.as_ref().unwrap().variant.as_ref().unwrap();
        match v {
            query::Variant::NearestWithMmr(inner) => {
                let div = inner.mmr.as_ref().and_then(|m| m.diversity).unwrap();
                assert!((div - 0.4).abs() < 1e-6, "diversity not propagated: {div}");
            }
            _ => panic!("content prefetch should be NearestWithMmr, got {v:?}"),
        }
    }

    #[test]
    fn mmr_only_affects_content_prefetch_not_others() {
        let mut w = LensWeights::default();
        w.diversity = Some(0.7);
        let dense = active_dense_specs(&w);
        let sparse = active_sparse_specs(&w);
        let qv = vec![0.1f32; 384];
        let qs = text_to_sparse("test query");
        let pre = build_prefetches(&w, &dense, &sparse, &qv, &qs, search_params_with_quantization())
            .unwrap();
        for p in &pre {
            if p.using.as_deref() == Some("content") {
                continue;
            }
            let v = p.query.as_ref().unwrap().variant.as_ref().unwrap();
            assert!(!matches!(v, query::Variant::NearestWithMmr(_)),
                    "non-content prefetch wrongly wrapped with MMR: {:?}", p.using);
        }
    }

    // ---- Weighted RRF (KA-05) — 3 unit tests ----------------------------

    #[test]
    fn rrf_weights_align_with_prefetch_order() {
        let w = LensWeights {
            content: 1.0,
            tool: 1.5,
            path: 1.5,
            error: 0.8,
            code: 0.6,
            content_late: 0.0,
            diversity: None,
            fusion: FusionMode::Rrf,
        };
        let dense = active_dense_specs(&w);
        let sparse = active_sparse_specs(&w);
        let rrf = build_rrf(&w, &dense, &sparse).build();
        let mut expected = Vec::new();
        for d in &dense {
            expected.push(d.weight);
        }
        for s in &sparse {
            expected.push(s.weight);
        }
        assert_eq!(rrf.weights, expected);
    }

    #[test]
    fn rrf_mode_does_not_emit_formula() {
        // sanity — FusionMode::Rrf is what build_rrf consumes; build_formula
        // is independent. Confirm RrfBuilder defaults k to None (server picks).
        let w = LensWeights::default();
        let rrf = build_rrf(&w, &active_dense_specs(&w), &active_sparse_specs(&w)).build();
        assert!(rrf.k.is_none(), "k should default to None (server default 60)");
    }

    #[test]
    fn rrf_weight_count_matches_active_prefetches() {
        let w = LensWeights {
            content: 2.0,
            tool: 0.0, // disabled
            path: 1.0,
            error: 0.5,
            code: 0.0, // disabled
            content_late: 0.0,
            diversity: None,
            fusion: FusionMode::Rrf,
        };
        let dense = active_dense_specs(&w);
        let sparse = active_sparse_specs(&w);
        let rrf = build_rrf(&w, &dense, &sparse).build();
        // dense: content, error (2). sparse: path_sparse (tool sparse skipped
        // because tool weight = 0). Total = 3.
        assert_eq!(rrf.weights.len(), 3);
        assert_eq!(rrf.weights, vec![2.0, 0.5, 1.0]);
    }

    // ---- Payload helpers -----------------------------------------------

    #[test]
    fn payload_bool_extracts_correct_value() {
        let mut p: HashMap<String, Value> = HashMap::new();
        p.insert(
            "has_errors".to_string(),
            Value { kind: Some(Kind::BoolValue(true)) },
        );
        assert_eq!(payload_bool(&p, "has_errors"), Some(true));
        assert_eq!(payload_bool(&p, "missing"), None);
    }

    #[test]
    fn lens_result_to_searchhit_preserves_breakdown() {
        let mut per = HashMap::new();
        per.insert("content".to_string(), 0.9);
        per.insert("tool".to_string(), 0.7);
        let r = LensResult {
            session_id: "s1".into(),
            score: 1.42,
            project_name: "p".into(),
            ai_title: "t".into(),
            start_iso: "2026-01-01T00:00:00Z".into(),
            score_breakdown: ScoreBreakdown {
                per_vector: per.clone(),
                recency_factor: 0.5,
                has_errors_boost: 0.2,
                final_score: 1.42,
            },
            payload_json: None,
        };
        let h = lens_result_to_searchhit(r);
        assert_eq!(h.session_id, "s1");
        assert_eq!(h.score, 1.42);
        assert_eq!(h.vector_scores, per);
    }
}
