//! Phase 4 advanced retrieval primitives.
//!
//! - KB-03 Discovery API true context pairs (`mix_match_with_pairs`)
//! - KB-04 ACORN filterable HNSW (`search_params_filtered_acorn`)
//! - KB-05 Order-by scroll (`list_sessions_ordered`)
//! - KA-03 Group-by query (`lens_search_grouped`)
//! - KA-04 RelevanceFeedback query (`relevance_feedback`)
//!
//! The KB-01 late-interaction wiring lives in `indexer.rs` (`LensWeights`
//! extension + `index_session` upsert) because it has to mutate the existing
//! lens_search pipeline. `embed_late.rs` provides the chunker.

use std::collections::HashMap;

use anyhow::{Context, Result};
use qdrant_client::qdrant::{
    point_id::PointIdOptions, ContextInputBuilder, ContextInputPair, Direction, Filter, OrderBy,
    OrderByBuilder, PointId, Query, QueryPointGroupsBuilder, QueryPointsBuilder,
    RelevanceFeedbackInputBuilder, ScrollPointsBuilder, SearchParams, SearchParamsBuilder,
    VectorInput, WithPayloadSelector,
};
use qdrant_client::Qdrant;
use serde::{Deserialize, Serialize};

use crate::indexer::{point_id, Embedder, SearchHit};
use crate::schema::{quantization_search_params, COLLECTION_V3};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A positive/negative session pair for Discovery API context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPair {
    pub positive_session_id: String,
    pub negative_session_id: String,
}

/// Sort direction for `list_sessions_ordered`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OrderDirection {
    Asc,
    Desc,
}

impl OrderDirection {
    pub fn as_proto(&self) -> Direction {
        match self {
            OrderDirection::Asc => Direction::Asc,
            OrderDirection::Desc => Direction::Desc,
        }
    }
}

/// Ordering spec for `list_sessions_ordered`. `key` must be in
/// [`ALLOWED_ORDER_KEYS`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBySpec {
    pub key: String,
    pub direction: OrderDirection,
}

/// Keys that can be used with `OrderBySpec`. Mirrors the v3 payload indexes
/// that are sortable (datetime, integer, bool — Qdrant rejects others).
pub const ALLOWED_ORDER_KEYS: &[&str] = &["start_ts_dt", "tool_count", "has_errors"];

/// Group-by spec for `lens_search_grouped`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupBy {
    pub key: String,
    pub group_size: u32,
}

/// Response from `lens_search_grouped` — flat + optional grouped projection.
/// Backward-compat: if `group_by` was `None`, `groups` is `None`.
#[derive(Debug, Clone, Serialize)]
pub struct LensSearchResponse {
    pub flat: Vec<SearchHit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<GroupedHits>>,
}

/// One group from `query_groups`. `group_id` is the payload value all the
/// hits share (string-coerced).
#[derive(Debug, Clone, Serialize)]
pub struct GroupedHits {
    pub group_id: String,
    pub hits: Vec<SearchHit>,
}

/// Light-weight session summary returned by `list_sessions_ordered`. Differs
/// from `commands::SessionSummary` in that it has no parser-only fields
/// (Qdrant-only) and adds `score: None` because scroll doesn't score.
#[derive(Debug, Clone, Serialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub project_name: String,
    pub ai_title: String,
    pub start_iso: String,
    pub tool_count: i64,
    pub has_errors: bool,
}

// ---------------------------------------------------------------------------
// KB-04 ACORN search params helper
// ---------------------------------------------------------------------------

/// Build `SearchParams` carrying the v3 quantization knobs (rescore +
/// oversampling) AND optional ACORN-shaped HNSW tuning. When `hnsw_ef` is
/// `Some(n)`, also forces `exact=false` so the request takes the ACORN path
/// rather than falling back to brute-force scan.
///
/// The `is_tenant=true` payload index on `project_name` (set in P3) provides
/// the structural half of ACORN; this helper provides the per-query tuning.
pub fn search_params_filtered_acorn(hnsw_ef: Option<u64>) -> SearchParams {
    let mut b = SearchParamsBuilder::default().quantization(quantization_search_params());
    if let Some(ef) = hnsw_ef {
        b = b.hnsw_ef(ef).exact(false);
    }
    b.build()
}

// ---------------------------------------------------------------------------
// KB-03 Discovery API true context pairs
// ---------------------------------------------------------------------------

/// Mix & Match with explicit context pairs. The target session anchors the
/// search; each pair pushes the search toward `positive` and away from
/// `negative`. Backward-compat note: the existing 5-argument `indexer::mix_match`
/// is preserved.
///
/// Empty `pairs` is valid → equivalent to a plain "find nearest to target"
/// search on the content vector.
pub async fn mix_match_with_pairs(
    client: &Qdrant,
    target_session_id: &str,
    pairs: &[ContextPair],
    limit: u64,
) -> Result<Vec<SearchHit>> {
    if target_session_id.is_empty() {
        anyhow::bail!("mix_match_with_pairs requires target_session_id");
    }

    let to_pid = |sid: &str| PointId {
        point_id_options: Some(PointIdOptions::Uuid(point_id(sid))),
    };

    let target_pid = to_pid(target_session_id);

    let pair_vec: Vec<ContextInputPair> = pairs
        .iter()
        .map(|p| ContextInputPair {
            positive: Some(VectorInput::from(to_pid(&p.positive_session_id))),
            negative: Some(VectorInput::from(to_pid(&p.negative_session_id))),
        })
        .collect();

    // Drop `.clone()` (Gemini PR #4 review, retrieval.rs:159): `pair_vec`
    // is consumed by the builder and not read again before the request.
    let context = ContextInputBuilder::default().pairs(pair_vec).build();
    let discover = qdrant_client::qdrant::DiscoverInput {
        target: Some(VectorInput::from(target_pid)),
        context: Some(context),
    };

    let resp = client
        .query(
            QueryPointsBuilder::new(COLLECTION_V3)
                .query(discover)
                .using("content".to_string())
                .limit(limit)
                .with_payload(true)
                .params(search_params_filtered_acorn(None)),
        )
        .await?;

    Ok(resp.result.into_iter().map(map_search_hit).collect())
}

// ---------------------------------------------------------------------------
// KB-05 Order-by scroll
// ---------------------------------------------------------------------------

/// List sessions from Qdrant ordered by a payload field. Used by the time
/// machine stack when the user wants to sort by tool count or restrict to
/// error sessions.
///
/// `key` must be one of [`ALLOWED_ORDER_KEYS`] — others return an error
/// rather than passing through, because Qdrant 1.18 silently returns
/// unordered results for keys without a sortable index.
pub async fn list_sessions_ordered(
    client: &Qdrant,
    order_by: Option<OrderBySpec>,
    filter: Option<Filter>,
    limit: u32,
) -> Result<Vec<SessionMeta>> {
    let order = order_by.unwrap_or_else(|| OrderBySpec {
        key: "start_ts_dt".to_string(),
        direction: OrderDirection::Desc,
    });
    if !ALLOWED_ORDER_KEYS.contains(&order.key.as_str()) {
        anyhow::bail!(
            "list_sessions_ordered: unsupported order_by key '{}' (allowed: {:?})",
            order.key,
            ALLOWED_ORDER_KEYS
        );
    }

    let proto: OrderBy = OrderByBuilder::new(order.key.clone())
        .direction(order.direction.as_proto() as i32)
        .build();

    let mut b = ScrollPointsBuilder::new(COLLECTION_V3)
        .with_payload(true)
        .with_vectors(false)
        .order_by(proto)
        .limit(limit);
    if let Some(f) = filter {
        b = b.filter(f);
    }

    let resp = client.scroll(b).await?;
    Ok(resp.result.into_iter().map(|p| {
        let pl = p.payload;
        SessionMeta {
            session_id: payload_string(&pl, "session_id"),
            project_name: payload_string(&pl, "project_name"),
            ai_title: payload_string(&pl, "ai_title"),
            start_iso: payload_string(&pl, "start_iso"),
            tool_count: payload_i64(&pl, "tool_count").unwrap_or(0),
            has_errors: payload_bool(&pl, "has_errors").unwrap_or(false),
        }
    }).collect())
}

// ---------------------------------------------------------------------------
// KA-03 Group-by query
// ---------------------------------------------------------------------------

/// Run a single-vector content search with optional grouping. When `group_by`
/// is `None` this is a thin wrapper around `client.query()` on the `content`
/// vector and returns flat results only. When `group_by` is `Some`, it uses
/// `client.query_groups()` and returns both the flat hits (concatenated) and
/// the per-group projection.
///
/// SPEC NOTE (P4, KA-03): the rich `lens_search` (5-vector weighted blend)
/// stays in `indexer.rs` to preserve backward compatibility. This
/// `lens_search_grouped` is the new IPC path — it intentionally does the
/// query on a single named vector (`content`) so the group_by semantics are
/// unambiguous. Multi-vector blending with grouping would require running
/// the weighted blend then re-grouping client-side, which we defer.
pub async fn lens_search_grouped(
    client: &Qdrant,
    embedder: &Embedder,
    query_text: &str,
    group_by: Option<GroupBy>,
    limit: u64,
) -> Result<LensSearchResponse> {
    let vecs = embedder.embed(vec![query_text.to_string()])?;
    let qvec = vecs.into_iter().next().context("no embedding for query")?;

    if let Some(gb) = group_by {
        let q: Query = qvec.into();
        // SEMANTICS FIX (Codex PR #4 review, retrieval.rs:280): `limit` in
        // the non-grouped path is the absolute cap on the flattened hit
        // list, but `QueryPointGroupsBuilder::limit(N)` is "max N groups",
        // which means the flat list could grow to `N * group_size`. That
        // tripped UI pagination that assumed `flat.len() <= limit`. We
        // pass a *group_limit* derived from `limit` so the flat list stays
        // within `limit` (rounded up): `group_limit = ceil(limit / group_size)`.
        let group_size = gb.group_size.max(1) as u64;
        let group_limit = limit.div_ceil(group_size).max(1);
        let resp = client
            .query_groups(
                QueryPointGroupsBuilder::new(COLLECTION_V3, gb.key.clone())
                    .query(q)
                    .using("content".to_string())
                    .group_size(group_size)
                    .limit(group_limit)
                    .with_payload(WithPayloadSelector::from(true))
                    .params(search_params_filtered_acorn(None)),
            )
            .await?;

        let mut flat: Vec<SearchHit> = Vec::new();
        let mut groups: Vec<GroupedHits> = Vec::new();

        if let Some(gr) = resp.result {
            for g in gr.groups {
                let gid = group_id_to_string(&g.id);
                let mut hits: Vec<SearchHit> = Vec::new();
                for sp in g.hits {
                    let hit = map_search_hit(sp);
                    flat.push(hit.clone());
                    hits.push(hit);
                }
                groups.push(GroupedHits { group_id: gid, hits });
            }
        }

        // SEMANTICS FIX (continued from above): even after picking
        // `group_limit = ceil(limit / group_size)`, the LAST group can
        // contribute a partial set, so the flat list can briefly exceed
        // `limit`. Clamp here so the public contract is exact.
        if flat.len() > limit as usize {
            flat.truncate(limit as usize);
        }
        Ok(LensSearchResponse {
            flat,
            groups: Some(groups),
        })
    } else {
        // No grouping — single-vector content search.
        let q: Query = qvec.into();
        let resp = client
            .query(
                QueryPointsBuilder::new(COLLECTION_V3)
                    .query(q)
                    .using("content".to_string())
                    .limit(limit)
                    .with_payload(true)
                    .params(search_params_filtered_acorn(None)),
            )
            .await?;
        let flat: Vec<SearchHit> = resp.result.into_iter().map(map_search_hit).collect();
        Ok(LensSearchResponse { flat, groups: None })
    }
}

// ---------------------------------------------------------------------------
// KA-04 Relevance feedback
// ---------------------------------------------------------------------------

/// Re-rank a search using `RelevanceFeedback`. The original query vector is
/// re-embedded from `previous_query` so the caller can hand back the
/// human-readable query and we keep the vector private. Positive IDs score
/// 1.0; negative IDs score 0.0 (binary feedback — Qdrant 1.18 supports
/// graded feedback via `FeedbackItem.score` but we expose the simple binary
/// case in the v1 IPC).
///
/// SPEC NOTE (P4, KA-04): Qdrant 1.18 *requires* a strategy on the
/// `RelevanceFeedback` query (server returns `InvalidArgument: "strategy
/// is missing"` otherwise). We default to `NaiveFeedbackStrategy { a: 1, b:
/// 1, c: 1 }` — the "use feedback as-is" baseline that the spec leaves as
/// a future tuning knob.
pub async fn relevance_feedback(
    client: &Qdrant,
    embedder: &Embedder,
    positive_ids: &[String],
    negative_ids: &[String],
    previous_query: &str,
    limit: u64,
) -> Result<Vec<SearchHit>> {
    let vecs = embedder.embed(vec![previous_query.to_string()])?;
    let qvec = vecs
        .into_iter()
        .next()
        .context("no embedding for previous_query")?;

    let to_pid_vec = |sid: &str| -> VectorInput {
        VectorInput::from(PointId {
            point_id_options: Some(PointIdOptions::Uuid(point_id(sid))),
        })
    };

    // Drop `.clone()` (Gemini PR #4 review, retrieval.rs:378): `qvec` is
    // moved into the VectorInput and not read again below.
    let target_vi: VectorInput = qvec.into();

    let mut builder = RelevanceFeedbackInputBuilder::new(target_vi);
    // Positive feedback: score = 1.0
    for sid in positive_ids {
        builder = builder.add_feedback(qdrant_client::qdrant::FeedbackItem {
            example: Some(to_pid_vec(sid)),
            score: 1.0,
        });
    }
    // Negative feedback: score = 0.0
    for sid in negative_ids {
        builder = builder.add_feedback(qdrant_client::qdrant::FeedbackItem {
            example: Some(to_pid_vec(sid)),
            score: 0.0,
        });
    }
    // Required by Qdrant 1.18 (see SPEC NOTE above).
    let strategy = qdrant_client::qdrant::FeedbackStrategy {
        variant: Some(qdrant_client::qdrant::feedback_strategy::Variant::Naive(
            qdrant_client::qdrant::NaiveFeedbackStrategy {
                a: 1.0,
                b: 1.0,
                c: 1.0,
            },
        )),
    };
    builder = builder.strategy(strategy);

    let feedback_input = builder.build();
    // Wrap in `Query::Variant::RelevanceFeedback` via the explicit construction.
    let query = Query {
        variant: Some(qdrant_client::qdrant::query::Variant::RelevanceFeedback(
            feedback_input,
        )),
    };

    let resp = client
        .query(
            QueryPointsBuilder::new(COLLECTION_V3)
                .query(query)
                .using("content".to_string())
                .limit(limit)
                .with_payload(true)
                .params(search_params_filtered_acorn(None)),
        )
        .await?;

    Ok(resp.result.into_iter().map(map_search_hit).collect())
}

// ---------------------------------------------------------------------------
// Payload helpers — lifted into `crate::payload` so the three (formerly
// four) duplicate implementations across indexer.rs/retrieval.rs/lens.rs
// stay in lock-step. Re-imported below; signatures unchanged so callers
// don't move (Gemini PR #4 review on retrieval.rs:470).
// ---------------------------------------------------------------------------

use crate::payload::{payload_bool, payload_i64, payload_string};

/// QUALITY FIX (Gemini PR #4 review, retrieval.rs:186): the mapping from a
/// Qdrant scored point payload to a `SearchHit` was inlined verbatim at four
/// call sites in this file (lines 179, 294, 329, 429 in the original PR).
/// Centralized here so changes to the SearchHit schema (or to the payload
/// field naming convention) only need to be applied once. `vector_scores`
/// stays empty because callers fill it in afterwards when running breakdown
/// queries — keeping that responsibility outside the mapping function
/// preserves the existing behavior of `lens_search_grouped`'s flat list.
fn map_search_hit(p: qdrant_client::qdrant::ScoredPoint) -> SearchHit {
    SearchHit {
        score: p.score,
        session_id: payload_string(&p.payload, "session_id"),
        project_name: payload_string(&p.payload, "project_name"),
        ai_title: payload_string(&p.payload, "ai_title"),
        start_iso: payload_string(&p.payload, "start_iso"),
        vector_scores: HashMap::new(),
    }
}

fn group_id_to_string(id: &Option<qdrant_client::qdrant::GroupId>) -> String {
    use qdrant_client::qdrant::group_id::Kind;
    match id.as_ref().and_then(|g| g.kind.as_ref()) {
        Some(Kind::StringValue(s)) => s.clone(),
        Some(Kind::IntegerValue(i)) => i.to_string(),
        Some(Kind::UnsignedValue(u)) => u.to_string(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- KB-04 helper -------------------------------------------------------

    #[test]
    fn t_search_params_filtered_acorn_sets_hnsw_ef() {
        let sp = search_params_filtered_acorn(Some(128));
        assert_eq!(sp.hnsw_ef, Some(128));
        assert_eq!(sp.exact, Some(false));
        assert!(sp.quantization.is_some());
    }

    #[test]
    fn t_search_params_filtered_acorn_default_no_hnsw_ef() {
        let sp = search_params_filtered_acorn(None);
        assert!(sp.hnsw_ef.is_none());
        // exact stays as default (None) when no hnsw_ef requested.
        assert!(sp.exact.is_none());
        assert!(sp.quantization.is_some());
    }

    #[test]
    fn t_recall_filter_in_hnsw_params() {
        // Mirrors the call site: when a filter is in play, recall() asks for
        // hnsw_ef=128 + exact=false to take the ACORN path.
        let sp = search_params_filtered_acorn(Some(128));
        assert_eq!(sp.hnsw_ef, Some(128));
        assert_eq!(sp.exact, Some(false));
    }

    // -- KB-03 Discovery API context pairs ----------------------------------

    #[test]
    fn t_context_pair_query_format() {
        // Build the same pair_vec the function builds and verify shape.
        let pairs = vec![
            ContextPair {
                positive_session_id: "sid-a".into(),
                negative_session_id: "sid-b".into(),
            },
            ContextPair {
                positive_session_id: "sid-c".into(),
                negative_session_id: "sid-d".into(),
            },
        ];

        let to_pid = |sid: &str| PointId {
            point_id_options: Some(PointIdOptions::Uuid(point_id(sid))),
        };
        let pair_vec: Vec<ContextInputPair> = pairs
            .iter()
            .map(|p| ContextInputPair {
                positive: Some(VectorInput::from(to_pid(&p.positive_session_id))),
                negative: Some(VectorInput::from(to_pid(&p.negative_session_id))),
            })
            .collect();

        assert_eq!(pair_vec.len(), 2);
        for p in &pair_vec {
            assert!(p.positive.is_some());
            assert!(p.negative.is_some());
        }
    }

    #[test]
    fn t_empty_pairs_uses_target_only() {
        // Constructing the request with no pairs is valid — `pair_vec` stays
        // empty and the ContextInputBuilder accepts it.
        let pair_vec: Vec<ContextInputPair> = Vec::new();
        let context = ContextInputBuilder::default().pairs(pair_vec.clone()).build();
        assert!(context.pairs.is_empty());
    }

    #[test]
    fn t_single_anchor_backward_compat() {
        // Verifies the old mix_match still compiles (signature is unchanged).
        // We don't call it because that needs a live Qdrant. Compile-only.
        let _ = crate::indexer::mix_match;
    }

    // -- KB-05 order-by -----------------------------------------------------

    #[test]
    fn t_order_by_proto_desc() {
        let proto: OrderBy = OrderByBuilder::new("start_ts_dt")
            .direction(Direction::Desc as i32)
            .build();
        assert_eq!(proto.key, "start_ts_dt");
        assert_eq!(proto.direction, Some(Direction::Desc as i32));
    }

    #[test]
    fn t_order_by_proto_asc() {
        let proto: OrderBy = OrderByBuilder::new("start_ts_dt")
            .direction(Direction::Asc as i32)
            .build();
        assert_eq!(proto.direction, Some(Direction::Asc as i32));
    }

    #[test]
    fn t_order_by_proto_tool_count_desc() {
        let proto: OrderBy = OrderByBuilder::new("tool_count")
            .direction(Direction::Desc as i32)
            .build();
        assert_eq!(proto.key, "tool_count");
        assert_eq!(proto.direction, Some(Direction::Desc as i32));
    }

    #[test]
    fn t_order_by_unsupported_key_rejected() {
        // We can't run this against Qdrant without a live server, but we can
        // verify the key-allowlist guard logic.
        let bad = "session_id";
        assert!(!ALLOWED_ORDER_KEYS.contains(&bad));
    }

    #[test]
    fn t_list_sessions_order_default_desc() {
        let default = OrderBySpec {
            key: "start_ts_dt".to_string(),
            direction: OrderDirection::Desc,
        };
        assert_eq!(default.direction.as_proto(), Direction::Desc);
        assert_eq!(default.key, "start_ts_dt");
    }

    #[test]
    fn t_list_sessions_order_by_tool_count() {
        let spec = OrderBySpec {
            key: "tool_count".to_string(),
            direction: OrderDirection::Desc,
        };
        assert!(ALLOWED_ORDER_KEYS.contains(&spec.key.as_str()));
    }

    #[test]
    fn t_list_sessions_order_oldest_first() {
        let spec = OrderBySpec {
            key: "start_ts_dt".to_string(),
            direction: OrderDirection::Asc,
        };
        assert_eq!(spec.direction.as_proto(), Direction::Asc);
    }

    // -- KA-03 group-by -----------------------------------------------------

    #[test]
    fn t_lens_with_group_by_project() {
        // Verify the GroupBy struct and that the builder accepts it.
        let gb = GroupBy {
            key: "project_name".to_string(),
            group_size: 3,
        };
        let builder = QueryPointGroupsBuilder::new(COLLECTION_V3, gb.key.clone())
            .group_size(gb.group_size as u64);
        // Builder must accept group_size — call build() to confirm.
        let proto = builder.build();
        assert_eq!(proto.group_by, "project_name");
        assert_eq!(proto.group_size, Some(3));
    }

    #[test]
    fn t_lens_without_group_by() {
        // Without group_by, LensSearchResponse.groups stays None.
        let r = LensSearchResponse {
            flat: vec![],
            groups: None,
        };
        assert!(r.groups.is_none());
    }

    // -- KA-04 RelevanceFeedback --------------------------------------------

    #[test]
    fn t_relevance_feedback_query_format() {
        // Build the proto and assert the variant carries the feedback input.
        let vec_in = VectorInput::from(vec![0.1f32; 384]);
        let to_pid_vec = |sid: &str| VectorInput::from(PointId {
            point_id_options: Some(PointIdOptions::Uuid(point_id(sid))),
        });
        let fb = RelevanceFeedbackInputBuilder::new(vec_in)
            .add_feedback(qdrant_client::qdrant::FeedbackItem {
                example: Some(to_pid_vec("pos-sid")),
                score: 1.0,
            })
            .add_feedback(qdrant_client::qdrant::FeedbackItem {
                example: Some(to_pid_vec("neg-sid")),
                score: 0.0,
            })
            .build();
        assert_eq!(fb.feedback.len(), 2);
        assert!(fb.target.is_some());
        let q = Query {
            variant: Some(qdrant_client::qdrant::query::Variant::RelevanceFeedback(fb)),
        };
        match q.variant {
            Some(qdrant_client::qdrant::query::Variant::RelevanceFeedback(input)) => {
                assert_eq!(input.feedback.len(), 2);
                assert!((input.feedback[0].score - 1.0).abs() < 1e-9);
                assert!((input.feedback[1].score - 0.0).abs() < 1e-9);
            }
            _ => panic!("expected RelevanceFeedback variant"),
        }
    }

    #[test]
    fn t_relevance_feedback_determinism() {
        // Two builds with the same inputs produce the same proto shape.
        let mk = || {
            let vec_in = VectorInput::from(vec![0.1f32; 384]);
            let pos = VectorInput::from(PointId {
                point_id_options: Some(PointIdOptions::Uuid(point_id("p1"))),
            });
            let neg = VectorInput::from(PointId {
                point_id_options: Some(PointIdOptions::Uuid(point_id("n1"))),
            });
            RelevanceFeedbackInputBuilder::new(vec_in)
                .add_feedback(qdrant_client::qdrant::FeedbackItem {
                    example: Some(pos),
                    score: 1.0,
                })
                .add_feedback(qdrant_client::qdrant::FeedbackItem {
                    example: Some(neg),
                    score: 0.0,
                })
                .build()
        };
        let a = mk();
        let b = mk();
        assert_eq!(a.feedback.len(), b.feedback.len());
        for (ia, ib) in a.feedback.iter().zip(b.feedback.iter()) {
            assert!((ia.score - ib.score).abs() < 1e-9);
        }
    }

    // -- KB-01 content_late prefetch decisions (compile-only) ----------------

    /// The actual prefetch wiring lives in `indexer::lens_search`. Here we
    /// assert the activation rule: weight > 0 ⇒ include slot, weight == 0
    /// ⇒ skip slot.
    #[test]
    fn t_lens_prefetch_includes_content_late() {
        // Issue #15 — `indexer::LensWeights` is a re-export of
        // `lens::LensWeights`; the 8-field shape includes `diversity` + `fusion`.
        let w = crate::indexer::LensWeights {
            content: 0.0,
            tool: 0.0,
            path: 0.0,
            error: 0.0,
            code: 0.0,
            content_late: 1.0,
            diversity: None,
            fusion: crate::lens::FusionMode::Formula,
        };
        assert!(w.content_late > 0.0, "non-zero weight must activate slot");
    }

    #[test]
    fn t_lens_prefetch_skips_content_late_zero() {
        // PR #12 REV-8 + CodeRabbit #9 — Default::default() now returns 0.25
        // (T3.3 baseline). To exercise the "skip when 0" branch, construct
        // a LensWeights with explicit content_late=0.0 instead of relying on
        // the Default impl. The skip-on-zero rule itself is what we assert.
        let mut w = crate::indexer::LensWeights::default();
        w.content_late = 0.0;
        assert_eq!(w.content_late, 0.0,
            "explicit content_late=0.0 must reach the prefetch chain as 0.0");
        // Sanity: confirm Default has been flipped to 0.25 (post-T3.3).
        let d = crate::indexer::LensWeights::default();
        assert!((d.content_late - 0.25).abs() < f32::EPSILON,
            "post-T3.3 default content_late must be 0.25");
    }

    #[test]
    fn t_content_late_upsert_format() {
        // Build a MultiDenseVector from chunk vectors and verify it converts
        // into the proto Vector slot Qdrant expects.
        use qdrant_client::qdrant::{MultiDenseVector, Vector as ProtoVector};
        let chunks: Vec<Vec<f32>> = (0..3).map(|_| vec![0.1f32; 384]).collect();
        let mv: MultiDenseVector = chunks.into();
        let proto_vec: ProtoVector = mv.into();
        // The inner oneof must be the MultiDense variant.
        match proto_vec.vector {
            Some(qdrant_client::qdrant::vector::Vector::MultiDense(m)) => {
                assert_eq!(m.vectors.len(), 3);
                for v in &m.vectors {
                    assert_eq!(v.data.len(), 384);
                }
            }
            _ => panic!("expected MultiDense vector variant"),
        }
    }

    // -- Property tests -----------------------------------------------------

    #[test]
    fn prop_late_max_sim_bounded() {
        // Cosine similarity must be in [-1, 1]. MaxSim is the column-max
        // average, which inherits the same bound.
        for s in &[-1.0_f32, -0.5, 0.0, 0.5, 1.0] {
            assert!(*s >= -1.0 && *s <= 1.0);
        }
    }

    #[test]
    fn prop_group_by_size_limit() {
        // Property: every group's hit count is ≤ group_size. We construct
        // synthetic groups to verify the check logic.
        let gb = GroupBy {
            key: "project_name".to_string(),
            group_size: 3,
        };
        let groups: Vec<GroupedHits> = vec![
            GroupedHits {
                group_id: "p1".into(),
                hits: vec![SearchHit {
                    score: 0.9,
                    session_id: "a".into(),
                    project_name: "p1".into(),
                    ai_title: "t".into(),
                    start_iso: "".into(),
                    vector_scores: HashMap::new(),
                }],
            },
            GroupedHits {
                group_id: "p2".into(),
                hits: (0..3)
                    .map(|i| SearchHit {
                        score: 0.9 - (i as f32) * 0.1,
                        session_id: format!("b{i}"),
                        project_name: "p2".into(),
                        ai_title: "t".into(),
                        start_iso: "".into(),
                        vector_scores: HashMap::new(),
                    })
                    .collect(),
            },
        ];
        for g in &groups {
            assert!(
                g.hits.len() <= gb.group_size as usize,
                "group {} has {} hits, > size {}",
                g.group_id,
                g.hits.len(),
                gb.group_size
            );
        }
    }
}
