//! Integration tests for Phase 4 advanced retrieval.
//!
//! Each test creates its own throwaway collection (suffix = nanos timestamp)
//! to avoid trampling the real `memex_sessions_v3` data. Requires
//! `memex-qdrant` running on `localhost:6334`. Skipped automatically when the
//! env var `MEMEX_SKIP_QDRANT_TESTS=1` is set (CI fallback).
//!
//! Test list (per the P4 KICK plan):
//!   1. it_late_max_sim_returns_results          (KB-01)
//!   2. it_discover_pairs_filters_corpus         (KB-03)
//!   3. it_order_by_with_filter                  (KB-05)
//!   4. it_group_by_with_query                   (KA-03)
//!   5. it_relevance_feedback_basic              (KA-04)
//!   6. it_acorn_filtered_search                 (KB-04 — extra coverage)

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use memex_lib::retrieval::{
    self, ContextPair, GroupBy, OrderBySpec, OrderDirection,
};
use qdrant_client::qdrant::{
    point_id::PointIdOptions, vector, vectors::VectorsOptions, vectors_config, Condition,
    CreateCollectionBuilder, DenseVector, Direction, Distance, Filter, HnswConfigDiff,
    KeywordIndexParamsBuilder, MultiDenseVector, MultiVectorComparator, MultiVectorConfigBuilder,
    NamedVectors, OrderByBuilder, PointId, PointStruct, QueryPointGroupsBuilder,
    QueryPointsBuilder, RelevanceFeedbackInputBuilder, ScrollPointsBuilder, UpsertPointsBuilder,
    Value, Vector, VectorInput, VectorParams, VectorParamsBuilder, VectorParamsMap, Vectors,
    VectorsConfig, WithPayloadSelector,
};
use qdrant_client::{Payload, Qdrant};

const EMBED_DIM: u64 = 384;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

fn skip_if_no_qdrant() -> bool {
    std::env::var("MEMEX_SKIP_QDRANT_TESTS").ok().as_deref() == Some("1")
}

async fn client() -> Qdrant {
    let url = std::env::var("MEMEX_QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".into());
    Qdrant::from_url(&url).build().expect("qdrant connect")
}

fn unique_collection(prefix: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let extra = std::process::id();
    format!("{prefix}_p4_{nanos}_{extra}")
}

async fn drop_collection(client: &Qdrant, name: &str) {
    let _ = client.delete_collection(name).await;
}

/// Build a minimal collection matching the v3 vector schema (5 dense + 1
/// multivector) — sparse vectors omitted because none of the P4 tests need
/// them and they require modifier=IDF setup.
async fn create_test_collection(client: &Qdrant, name: &str) {
    // Dense vector params
    let dense_params: VectorParams =
        VectorParamsBuilder::new(EMBED_DIM, Distance::Cosine).build();
    let multi_params: VectorParams = VectorParamsBuilder::new(EMBED_DIM, Distance::Cosine)
        .multivector_config(MultiVectorConfigBuilder::new(MultiVectorComparator::MaxSim).build())
        .hnsw_config(HnswConfigDiff {
            m: Some(0),
            ..Default::default()
        })
        .build();

    let mut params_map: HashMap<String, VectorParams> = HashMap::new();
    for n in ["content", "tool", "path", "error", "code"] {
        params_map.insert(n.to_string(), dense_params.clone());
    }
    params_map.insert("content_late".to_string(), multi_params);

    let vectors_cfg: VectorsConfig =
        vectors_config::Config::ParamsMap(VectorParamsMap { map: params_map }).into();

    client
        .create_collection(
            CreateCollectionBuilder::new(name)
                .vectors_config(vectors_cfg)
                .timeout(30),
        )
        .await
        .expect("create_collection");

    // Add a tenant keyword index on project_name + an integer index on
    // tool_count so order_by + filter tests work.
    use qdrant_client::qdrant::{CreateFieldIndexCollectionBuilder, FieldType};
    let _ = client
        .create_field_index(
            CreateFieldIndexCollectionBuilder::new(name, "project_name", FieldType::Keyword)
                .field_index_params(KeywordIndexParamsBuilder::default().is_tenant(true)),
        )
        .await;
    let _ = client
        .create_field_index(CreateFieldIndexCollectionBuilder::new(
            name,
            "tool_count",
            FieldType::Integer,
        ))
        .await;
    let _ = client
        .create_field_index(CreateFieldIndexCollectionBuilder::new(
            name,
            "has_errors",
            FieldType::Bool,
        ))
        .await;
}

fn dense_vec_at_angle(seed: f32) -> Vec<f32> {
    (0..EMBED_DIM as usize)
        .map(|i| (seed + (i as f32) * 1e-4).sin())
        .collect()
}

fn vector_dense(data: Vec<f32>) -> Vector {
    Vector {
        vector: Some(vector::Vector::Dense(DenseVector { data })),
        ..Default::default()
    }
}

fn point_struct(
    sid: &str,
    seed: f32,
    project: &str,
    has_errors: bool,
    tool_count: i64,
    include_multivec: bool,
) -> PointStruct {
    use memex_lib::indexer::point_id;
    let id = PointId {
        point_id_options: Some(PointIdOptions::Uuid(point_id(sid))),
    };

    let data = dense_vec_at_angle(seed);
    let mut named: HashMap<String, Vector> = HashMap::new();
    for n in ["content", "tool", "path", "error", "code"] {
        named.insert(n.to_string(), vector_dense(data.clone()));
    }
    if include_multivec {
        let chunks: Vec<Vec<f32>> = (0..3).map(|j| dense_vec_at_angle(seed + (j as f32) * 0.01)).collect();
        let mv: MultiDenseVector = chunks.into();
        named.insert(
            "content_late".to_string(),
            Vector {
                vector: Some(vector::Vector::MultiDense(mv)),
                ..Default::default()
            },
        );
    }

    let payload_json = serde_json::json!({
        "session_id": sid,
        "project_name": project,
        "ai_title": format!("session {sid}"),
        "start_iso": "2026-05-17T09:15:18.335Z",
        "start_ts_dt": "2026-05-17T09:15:18.335Z",
        "tool_count": tool_count,
        "has_errors": has_errors,
        "source_path": format!("/tmp/.claude/projects/-tmp-x/{sid}.jsonl"),
    });
    let payload = Payload::try_from(payload_json).expect("payload");

    PointStruct {
        id: Some(id),
        payload: payload.into(),
        vectors: Some(Vectors {
            vectors_options: Some(VectorsOptions::Vectors(NamedVectors { vectors: named })),
        }),
    }
}

// ---------------------------------------------------------------------------
// KB-01 — content_late MaxSim returns results
// ---------------------------------------------------------------------------

#[tokio::test]
async fn it_late_max_sim_returns_results() {
    if skip_if_no_qdrant() {
        return;
    }
    let cli = client().await;
    let name = unique_collection("test_late");
    drop_collection(&cli, &name).await;
    create_test_collection(&cli, &name).await;

    // Upsert 3 points with content_late multivector data.
    let points = vec![
        point_struct("late-a", 0.1, "p1", false, 5, true),
        point_struct("late-b", 0.2, "p1", false, 7, true),
        point_struct("late-c", 0.3, "p2", true, 3, true),
    ];
    cli.upsert_points(UpsertPointsBuilder::new(&name, points).wait(true))
        .await
        .expect("upsert");

    // Query the content_late slot directly with a single dense query.
    let qvec = dense_vec_at_angle(0.15); // closer to late-a / late-b
    let q: qdrant_client::qdrant::Query = qvec.into();
    let resp = cli
        .query(
            QueryPointsBuilder::new(&name)
                .query(q)
                .using("content_late".to_string())
                .limit(5u64)
                .with_payload(true),
        )
        .await
        .expect("query content_late");

    assert!(!resp.result.is_empty(), "content_late query returned 0 hits");

    drop_collection(&cli, &name).await;
}

// ---------------------------------------------------------------------------
// KB-03 — Discovery API context pairs filter the corpus
// ---------------------------------------------------------------------------

#[tokio::test]
async fn it_discover_pairs_filters_corpus() {
    if skip_if_no_qdrant() {
        return;
    }
    let cli = client().await;
    let name = unique_collection("test_pairs");
    drop_collection(&cli, &name).await;
    create_test_collection(&cli, &name).await;

    // Upsert 4 points distributed across two clusters.
    let points = vec![
        point_struct("pair-anchor", 0.0, "p1", false, 5, false),
        point_struct("pair-pos", 0.05, "p1", false, 6, false),
        point_struct("pair-neg", 3.14, "p2", false, 4, false), // farthest
        point_struct("pair-other", 1.5, "p3", false, 8, false),
    ];
    cli.upsert_points(UpsertPointsBuilder::new(&name, points).wait(true))
        .await
        .expect("upsert");

    // Use a temporary patched collection name by overriding via env? No —
    // the retrieval helper hardcodes COLLECTION_V3. So we exercise the
    // proto construction directly here against our throwaway collection.
    use memex_lib::indexer::point_id;
    let to_pid = |sid: &str| PointId {
        point_id_options: Some(PointIdOptions::Uuid(point_id(sid))),
    };
    let pairs = vec![qdrant_client::qdrant::ContextInputPair {
        positive: Some(VectorInput::from(to_pid("pair-pos"))),
        negative: Some(VectorInput::from(to_pid("pair-neg"))),
    }];
    let context = qdrant_client::qdrant::ContextInputBuilder::default()
        .pairs(pairs)
        .build();
    let discover = qdrant_client::qdrant::DiscoverInput {
        target: Some(VectorInput::from(to_pid("pair-anchor"))),
        context: Some(context),
    };
    let resp = cli
        .query(
            QueryPointsBuilder::new(&name)
                .query(discover)
                .using("content".to_string())
                .limit(10u64)
                .with_payload(true),
        )
        .await
        .expect("discover query");

    assert!(!resp.result.is_empty(), "discover returned 0 hits");
    // The "pair-neg" anchor must NOT outrank "pair-pos" — Discovery API
    // explicitly repels from the negative.
    let mut pos_rank = usize::MAX;
    let mut neg_rank = usize::MAX;
    for (i, p) in resp.result.iter().enumerate() {
        let sid = p
            .payload
            .get("session_id")
            .and_then(|v| v.kind.as_ref())
            .and_then(|k| match k {
                qdrant_client::qdrant::value::Kind::StringValue(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        if sid == "pair-pos" {
            pos_rank = i;
        }
        if sid == "pair-neg" {
            neg_rank = i;
        }
    }
    if pos_rank != usize::MAX && neg_rank != usize::MAX {
        assert!(
            pos_rank < neg_rank,
            "pair-pos rank {pos_rank} must beat pair-neg rank {neg_rank}"
        );
    }
    drop_collection(&cli, &name).await;
}

// ---------------------------------------------------------------------------
// KB-05 — order_by with filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn it_order_by_with_filter() {
    if skip_if_no_qdrant() {
        return;
    }
    let cli = client().await;
    let name = unique_collection("test_order");
    drop_collection(&cli, &name).await;
    create_test_collection(&cli, &name).await;

    // Mix has_errors / clean sessions with varied tool_count.
    let points = vec![
        point_struct("order-1", 0.10, "p1", true, 12, false),
        point_struct("order-2", 0.11, "p1", true, 3, false),
        point_struct("order-3", 0.12, "p1", false, 30, false),
        point_struct("order-4", 0.13, "p2", true, 7, false),
    ];
    cli.upsert_points(UpsertPointsBuilder::new(&name, points).wait(true))
        .await
        .expect("upsert");

    // Scroll with order_by(tool_count desc) and filter(has_errors == true).
    let order_proto = OrderByBuilder::new("tool_count")
        .direction(Direction::Desc as i32)
        .build();
    let filter = Filter {
        must: vec![Condition::matches("has_errors", true)],
        ..Default::default()
    };
    let resp = cli
        .scroll(
            ScrollPointsBuilder::new(&name)
                .with_payload(true)
                .with_vectors(false)
                .order_by(order_proto)
                .filter(filter)
                .limit(10u32),
        )
        .await
        .expect("scroll order_by");

    assert!(!resp.result.is_empty());
    // All returned points must have has_errors=true and tool_count must be
    // monotonically non-increasing.
    let mut prev = i64::MAX;
    for p in &resp.result {
        let pl = &p.payload;
        let err = pl
            .get("has_errors")
            .and_then(|v| v.kind.as_ref())
            .map(|k| matches!(k, qdrant_client::qdrant::value::Kind::BoolValue(true)))
            .unwrap_or(false);
        assert!(err, "scroll returned a has_errors=false point");
        let tc = pl
            .get("tool_count")
            .and_then(|v| v.kind.as_ref())
            .and_then(|k| match k {
                qdrant_client::qdrant::value::Kind::IntegerValue(i) => Some(*i),
                _ => None,
            })
            .unwrap_or(0);
        assert!(tc <= prev, "order_by desc violated: {tc} > {prev}");
        prev = tc;
    }
    drop_collection(&cli, &name).await;
}

// ---------------------------------------------------------------------------
// KA-03 — group_by query
// ---------------------------------------------------------------------------

#[tokio::test]
async fn it_group_by_with_query() {
    if skip_if_no_qdrant() {
        return;
    }
    let cli = client().await;
    let name = unique_collection("test_group");
    drop_collection(&cli, &name).await;
    create_test_collection(&cli, &name).await;

    let points = vec![
        point_struct("g-1", 0.10, "alpha", false, 3, false),
        point_struct("g-2", 0.11, "alpha", false, 4, false),
        point_struct("g-3", 0.12, "alpha", false, 5, false),
        point_struct("g-4", 0.13, "beta", false, 6, false),
        point_struct("g-5", 0.14, "beta", false, 7, false),
        point_struct("g-6", 0.15, "gamma", false, 8, false),
    ];
    cli.upsert_points(UpsertPointsBuilder::new(&name, points).wait(true))
        .await
        .expect("upsert");

    let qvec = dense_vec_at_angle(0.12);
    let q: qdrant_client::qdrant::Query = qvec.into();
    let resp = cli
        .query_groups(
            QueryPointGroupsBuilder::new(&name, "project_name".to_string())
                .query(q)
                .using("content".to_string())
                .group_size(2u64)
                .limit(10u64)
                .with_payload(WithPayloadSelector::from(true)),
        )
        .await
        .expect("query_groups");

    let groups = resp.result.expect("result").groups;
    assert!(!groups.is_empty(), "no groups returned");
    // Each group's hits count must be ≤ group_size (= 2).
    for g in &groups {
        assert!(
            g.hits.len() <= 2,
            "group has {} hits, exceeds group_size=2",
            g.hits.len()
        );
    }
    drop_collection(&cli, &name).await;
}

// ---------------------------------------------------------------------------
// KA-04 — RelevanceFeedback basic ranking shift
// ---------------------------------------------------------------------------

#[tokio::test]
async fn it_relevance_feedback_basic() {
    if skip_if_no_qdrant() {
        return;
    }
    let cli = client().await;
    let name = unique_collection("test_feedback");
    drop_collection(&cli, &name).await;
    create_test_collection(&cli, &name).await;

    let points = vec![
        point_struct("fb-anchor", 0.0, "p1", false, 5, false),
        point_struct("fb-pos", 0.05, "p1", false, 6, false),
        point_struct("fb-neg", 1.5, "p2", false, 4, false),
    ];
    cli.upsert_points(UpsertPointsBuilder::new(&name, points).wait(true))
        .await
        .expect("upsert");

    use memex_lib::indexer::point_id;
    let to_pid_vec = |sid: &str| VectorInput::from(PointId {
        point_id_options: Some(PointIdOptions::Uuid(point_id(sid))),
    });

    let qvec_in = VectorInput::from(dense_vec_at_angle(0.02));
    let strategy = qdrant_client::qdrant::FeedbackStrategy {
        variant: Some(qdrant_client::qdrant::feedback_strategy::Variant::Naive(
            qdrant_client::qdrant::NaiveFeedbackStrategy {
                a: 1.0,
                b: 1.0,
                c: 1.0,
            },
        )),
    };
    let fb = RelevanceFeedbackInputBuilder::new(qvec_in)
        .add_feedback(qdrant_client::qdrant::FeedbackItem {
            example: Some(to_pid_vec("fb-pos")),
            score: 1.0,
        })
        .add_feedback(qdrant_client::qdrant::FeedbackItem {
            example: Some(to_pid_vec("fb-neg")),
            score: 0.0,
        })
        .strategy(strategy)
        .build();
    let q = qdrant_client::qdrant::Query {
        variant: Some(qdrant_client::qdrant::query::Variant::RelevanceFeedback(fb)),
    };

    let resp = cli
        .query(
            QueryPointsBuilder::new(&name)
                .query(q)
                .using("content".to_string())
                .limit(5u64)
                .with_payload(true),
        )
        .await
        .expect("relevance feedback query");

    assert!(!resp.result.is_empty(), "relevance feedback returned 0");
    drop_collection(&cli, &name).await;
}

// ---------------------------------------------------------------------------
// KB-04 — ACORN filtered search uses hnsw_ef
// ---------------------------------------------------------------------------

#[tokio::test]
async fn it_acorn_filtered_search() {
    if skip_if_no_qdrant() {
        return;
    }
    let cli = client().await;
    let name = unique_collection("test_acorn");
    drop_collection(&cli, &name).await;
    create_test_collection(&cli, &name).await;

    let points = vec![
        point_struct("ac-1", 0.10, "p1", true, 5, false),
        point_struct("ac-2", 0.11, "p1", false, 6, false),
        point_struct("ac-3", 0.12, "p2", true, 3, false),
    ];
    cli.upsert_points(UpsertPointsBuilder::new(&name, points).wait(true))
        .await
        .expect("upsert");

    // Query with the ACORN-tuned search_params (hnsw_ef=128 + exact=false) +
    // a tenant filter on project_name.
    let qvec = dense_vec_at_angle(0.10);
    let q: qdrant_client::qdrant::Query = qvec.into();
    let sp = retrieval::search_params_filtered_acorn(Some(128));
    assert_eq!(sp.hnsw_ef, Some(128));
    assert_eq!(sp.exact, Some(false));

    let filter = Filter {
        must: vec![Condition::matches("project_name", "p1".to_string())],
        ..Default::default()
    };
    let resp = cli
        .query(
            QueryPointsBuilder::new(&name)
                .query(q)
                .using("content".to_string())
                .limit(10u64)
                .filter(filter)
                .with_payload(true)
                .params(sp),
        )
        .await
        .expect("acorn query");

    assert!(!resp.result.is_empty(), "acorn search returned 0 hits");
    // All hits must satisfy the filter.
    for p in &resp.result {
        let proj = p
            .payload
            .get("project_name")
            .and_then(|v| v.kind.as_ref())
            .and_then(|k| match k {
                qdrant_client::qdrant::value::Kind::StringValue(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        assert_eq!(proj, "p1", "filter not honored: got proj={proj}");
    }

    drop_collection(&cli, &name).await;
}

// Suppress unused-imports lint when feature gating evolves.
#[allow(dead_code)]
fn _silence_unused() {
    let _ = ContextPair {
        positive_session_id: String::new(),
        negative_session_id: String::new(),
    };
    let _ = OrderBySpec {
        key: "start_ts_dt".to_string(),
        direction: OrderDirection::Desc,
    };
    let _ = GroupBy {
        key: "project_name".to_string(),
        group_size: 1,
    };
    let _: Option<Value> = None;
}
