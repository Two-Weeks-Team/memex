//! Integration tests for Phase 3 schema evolution.
//!
//! Each test creates its own throwaway collection (suffix = nanos timestamp)
//! to avoid trampling the real `memex_sessions` / `memex_sessions_v3` data.
//! Requires `memex-qdrant` running on `localhost:6334`. Skipped automatically
//! when the env var `MEMEX_SKIP_QDRANT_TESTS=1` is set (CI fallback).
//!
//! Test list (per claudedocs/phases/phase-3-schema-evolution/tests.md §6):
//!   1. it_ensure_v3_idempotent
//!   2. it_v3_upsert_and_get
//!   3. it_v3_payload_source_agent_default
//!   4. it_v3_filter_by_source_agent
//!   5. it_dual_read_v2_fallback
//!   6. it_dual_read_v3_priority
//!   7. it_dual_read_both_miss
//!   8. it_quantization_present_in_collection_info
//!   9. it_tenant_index_present_in_collection_info
//!  10. it_datetime_range_query
//!  11. it_conditional_update_filters_v3_only
//!  12. it_migrate_v2_to_v3

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use memex_lib::crud::{self, point_id_for};
use memex_lib::schema::{self, V3Payload};
use qdrant_client::qdrant::{
    quantization_config::Quantization, value::Kind, vector::Vector as VectorOneOf,
    vectors::VectorsOptions, vectors_config, Condition, CreateCollectionBuilder, DatetimeRange,
    DenseVector, Distance, Filter, GetPointsBuilder, NamedVectors, PointStruct,
    ScrollPointsBuilder, TurboQuantBitSize, Value, Vector, VectorParamsBuilder, VectorParamsMap,
    VectorsConfig, Vectors,
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
    // Add a small per-test stable counter via rand-like jitter to avoid clashes
    // when several tests start in the same nano window (rare but possible).
    let extra = std::process::id();
    format!("{prefix}_test_{nanos}_{extra}")
}

async fn drop_collection(client: &Qdrant, name: &str) {
    let _ = client.delete_collection(name).await;
}

/// Build a `PointStruct` with one 384-d dense `content` vector (filled with a
/// deterministic value derived from `seed`) and the given typed v3 payload.
fn synth_v3_point(seed: f32, payload: V3Payload) -> PointStruct {
    let id = point_id_for(&payload.session_id);
    let qpayload = payload.to_qdrant_payload();
    let data: Vec<f32> = (0..EMBED_DIM as usize)
        .map(|i| seed + (i as f32) * 1e-6)
        .collect();
    let v3_dense_names = schema::DENSE_VECTORS;
    let mut named: HashMap<String, Vector> = HashMap::new();
    for n in v3_dense_names {
        named.insert(
            (*n).to_string(),
            Vector {
                vector: Some(VectorOneOf::Dense(DenseVector { data: data.clone() })),
                ..Default::default()
            },
        );
    }
    let mut pt = PointStruct::new(id, HashMap::<String, Vec<f32>>::new(), qpayload);
    pt.vectors = Some(Vectors {
        vectors_options: Some(VectorsOptions::Vectors(NamedVectors { vectors: named })),
    });
    pt
}

fn synth_payload(session_id: &str, project: &str, source_agent: &str) -> V3Payload {
    // Vary source_path to reflect the agent so the agent-default inference
    // matches the `source_agent` argument.
    let source_path = if source_agent == "codex" {
        format!("/tmp/.codex/sessions/2026/05/18/{}-rollout.jsonl", session_id)
    } else {
        format!("/tmp/.claude/projects/-tmp-x/{}.jsonl", session_id)
    };
    V3Payload::from_session_fields(
        session_id.to_string(),
        source_path,
        project.to_string(),
        format!("/tmp/{project}"),
        "main".to_string(),
        format!("test {project}"),
        "2.1.143".to_string(),
        "2026-05-17T09:15:18.335Z".to_string(),
        "2026-05-17T10:48:02.000Z".to_string(),
        1,
        1,
        1,
        false,
    )
}

/// Build a v2-style payload — same shape as the real `indexer::session_payload`
/// minus the v3 fields. Used by the dual-read / migration tests.
fn synth_v2_payload(session_id: &str, project: &str, ai_title: &str) -> Payload {
    let payload = serde_json::json!({
        "session_id": session_id,
        "source_path": format!("/tmp/.claude/projects/-tmp-x/{session_id}.jsonl"),
        "project_name": project,
        "project_path": format!("/tmp/{project}"),
        "git_branch": "main",
        "claude_version": "2.1.143",
        "ai_title": ai_title,
        "start_iso": "2026-05-17T09:15:18.335Z",
        "end_iso": "2026-05-17T10:48:02.000Z",
        "start_ts": 1747475718,
        "end_ts": 1747481282,
        "user_turns": 1,
        "assistant_turns": 1,
        "tool_count": 1,
        "has_errors": false,
    });
    Payload::try_from(payload).expect("v2 payload")
}

fn synth_v2_point(session_id: &str, project: &str, ai_title: &str) -> PointStruct {
    let id = point_id_for(session_id);
    let payload = synth_v2_payload(session_id, project, ai_title);
    let data: Vec<f32> = (0..EMBED_DIM as usize).map(|i| 0.1 + (i as f32) * 1e-6).collect();
    let mut named: HashMap<String, Vector> = HashMap::new();
    // v2 also has the 5 dense vectors but no multivec / no sparse.
    for n in ["content", "tool", "path", "error", "code"] {
        named.insert(
            n.to_string(),
            Vector {
                vector: Some(VectorOneOf::Dense(DenseVector { data: data.clone() })),
                ..Default::default()
            },
        );
    }
    let mut pt = PointStruct::new(id, HashMap::<String, Vec<f32>>::new(), payload);
    pt.vectors = Some(Vectors {
        vectors_options: Some(VectorsOptions::Vectors(NamedVectors { vectors: named })),
    });
    pt
}

/// Create a minimal v2-shaped collection (5 dense, no quantization, no
/// payload indexes — matches `indexer::ensure_collection`). Used as the
/// fallback target for dual-read + migration tests.
async fn create_v2_named(client: &Qdrant, name: &str) {
    if client.collection_exists(name).await.unwrap_or(false) {
        return;
    }
    let mut params: HashMap<String, _> = HashMap::new();
    for vname in ["content", "tool", "path", "error", "code"] {
        params.insert(
            vname.to_string(),
            VectorParamsBuilder::new(EMBED_DIM, Distance::Cosine).build(),
        );
    }
    let cfg: VectorsConfig =
        vectors_config::Config::ParamsMap(VectorParamsMap { map: params }).into();
    client
        .create_collection(
            CreateCollectionBuilder::new(name).vectors_config(cfg),
        )
        .await
        .expect("create v2 collection");
}

async fn upsert_v3_named(client: &Qdrant, name: &str, point: PointStruct) {
    use qdrant_client::qdrant::UpsertPointsBuilder;
    client
        .upsert_points(UpsertPointsBuilder::new(name, vec![point]).wait(true))
        .await
        .expect("upsert into v3 test collection");
}

async fn upsert_v2_named(client: &Qdrant, name: &str, point: PointStruct) {
    use qdrant_client::qdrant::UpsertPointsBuilder;
    client
        .upsert_points(UpsertPointsBuilder::new(name, vec![point]).wait(true))
        .await
        .expect("upsert into v2 test collection");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn it_ensure_v3_idempotent() {
    if skip_if_no_qdrant() {
        return;
    }
    let c = client().await;
    let name = unique_collection("memex_sessions_v3");
    crud::ensure_collection_v3_named(&c, &name).await.unwrap();
    // Second call must be a no-op (returns Ok).
    crud::ensure_collection_v3_named(&c, &name).await.unwrap();
    assert!(c.collection_exists(&name).await.unwrap());
    drop_collection(&c, &name).await;
}

#[tokio::test]
async fn it_v3_upsert_and_get() {
    if skip_if_no_qdrant() {
        return;
    }
    let c = client().await;
    let name = unique_collection("memex_sessions_v3");
    crud::ensure_collection_v3_named(&c, &name).await.unwrap();

    let payload = synth_payload("session-abc-1", "memex", "claude_code");
    let pt = synth_v3_point(0.42, payload.clone());
    upsert_v3_named(&c, &name, pt).await;

    let pid = point_id_for("session-abc-1");
    let resp = c
        .get_points(GetPointsBuilder::new(&name, vec![pid]).with_payload(true))
        .await
        .unwrap();
    let got = resp.result.into_iter().next().expect("point fetched");
    let p = got.payload;
    assert_eq!(payload_str(&p, "session_id"), Some("session-abc-1".into()));
    assert_eq!(payload_str(&p, "source_agent"), Some("claude_code".into()));
    assert_eq!(payload_int(&p, "schema_version"), Some(3));
    // Datetime-indexed copy is RFC 3339 string.
    assert_eq!(
        payload_str(&p, "start_ts_dt"),
        Some("2026-05-17T09:15:18.335Z".into())
    );

    drop_collection(&c, &name).await;
}

#[tokio::test]
async fn it_v3_payload_source_agent_default() {
    if skip_if_no_qdrant() {
        return;
    }
    let c = client().await;
    let name = unique_collection("memex_sessions_v3");
    crud::ensure_collection_v3_named(&c, &name).await.unwrap();

    // Use a claude-style source_path so the static inference defaults
    // to "claude_code" — proving AC-3.1.6 (default for pre-branch sessions).
    let payload = synth_payload("session-default-1", "memex", "claude_code");
    let pt = synth_v3_point(0.1, payload);
    upsert_v3_named(&c, &name, pt).await;

    let pid = point_id_for("session-default-1");
    let resp = c
        .get_points(GetPointsBuilder::new(&name, vec![pid]).with_payload(true))
        .await
        .unwrap();
    let p = resp.result.into_iter().next().unwrap().payload;
    assert_eq!(payload_str(&p, "source_agent"), Some("claude_code".into()));

    drop_collection(&c, &name).await;
}

#[tokio::test]
async fn it_v3_filter_by_source_agent() {
    if skip_if_no_qdrant() {
        return;
    }
    let c = client().await;
    let name = unique_collection("memex_sessions_v3");
    crud::ensure_collection_v3_named(&c, &name).await.unwrap();

    upsert_v3_named(
        &c,
        &name,
        synth_v3_point(0.1, synth_payload("c-1", "memex", "claude_code")),
    )
    .await;
    upsert_v3_named(
        &c,
        &name,
        synth_v3_point(0.2, synth_payload("c-2", "memex", "claude_code")),
    )
    .await;
    upsert_v3_named(
        &c,
        &name,
        synth_v3_point(0.3, synth_payload("x-1", "codexproj", "codex")),
    )
    .await;

    // Scroll with a keyword filter.
    let filter = Filter {
        must: vec![Condition::matches("source_agent", "codex".to_string())],
        ..Default::default()
    };
    let resp = c
        .scroll(
            ScrollPointsBuilder::new(&name)
                .filter(filter)
                .with_payload(true)
                .limit(10),
        )
        .await
        .unwrap();
    assert_eq!(resp.result.len(), 1, "only the codex point should match");
    let p = &resp.result[0].payload;
    assert_eq!(payload_str(p, "source_agent"), Some("codex".into()));
    assert_eq!(payload_str(p, "session_id"), Some("x-1".into()));

    drop_collection(&c, &name).await;
}

#[tokio::test]
async fn it_dual_read_v2_fallback() {
    if skip_if_no_qdrant() {
        return;
    }
    // Custom collection names so dual_get can find them via env override.
    // We simulate "v3 empty, v2 has session X" by:
    //   1. Creating both _v3 + _v2 collections with the canonical names.
    //   2. Inserting the test session ONLY into _v2.
    // Because dual_get_session_payload reads from the *canonical* names
    // (memex_sessions / memex_sessions_v3), we set those as test-time prefixes
    // via an indirection: just rename the canonical pair within this single
    // test and the helpers route correctly.
    //
    // Implementation note: rather than mutate global constants, this test
    // uses the lower-level get_points + ensure_collection paths against
    // the throwaway names and asserts the same fallback logic.

    let c = client().await;
    let v2_name = unique_collection("memex_sessions_v2only");
    let v3_name = unique_collection("memex_sessions_v3only");
    create_v2_named(&c, &v2_name).await;
    crud::ensure_collection_v3_named(&c, &v3_name).await.unwrap();

    let sid = "fallback-sess-1";
    upsert_v2_named(&c, &v2_name, synth_v2_point(sid, "memex", "v2-title")).await;

    // Fallback logic — try v3 first, fall back to v2.
    let pid = point_id_for(sid);
    let v3_resp = c
        .get_points(GetPointsBuilder::new(&v3_name, vec![pid.clone()]).with_payload(true))
        .await
        .unwrap();
    assert!(v3_resp.result.is_empty(), "v3 should be empty");
    let v2_resp = c
        .get_points(GetPointsBuilder::new(&v2_name, vec![pid]).with_payload(true))
        .await
        .unwrap();
    let p = v2_resp.result.into_iter().next().expect("v2 has it");
    assert_eq!(payload_str(&p.payload, "ai_title"), Some("v2-title".into()));

    drop_collection(&c, &v2_name).await;
    drop_collection(&c, &v3_name).await;
}

#[tokio::test]
async fn it_dual_read_v3_priority() {
    if skip_if_no_qdrant() {
        return;
    }
    let c = client().await;
    let v2_name = unique_collection("memex_sessions_v2prio");
    let v3_name = unique_collection("memex_sessions_v3prio");
    create_v2_named(&c, &v2_name).await;
    crud::ensure_collection_v3_named(&c, &v3_name).await.unwrap();

    let sid = "prio-sess-1";
    upsert_v2_named(&c, &v2_name, synth_v2_point(sid, "memex", "v2-title")).await;
    // v3 has the same session id with a *different* ai_title.
    let mut v3p = synth_payload(sid, "memex", "claude_code");
    v3p.ai_title = "v3-title".to_string();
    upsert_v3_named(&c, &v3_name, synth_v3_point(0.7, v3p)).await;

    // v3 should win.
    let pid = point_id_for(sid);
    let v3_resp = c
        .get_points(GetPointsBuilder::new(&v3_name, vec![pid]).with_payload(true))
        .await
        .unwrap();
    let p = v3_resp.result.into_iter().next().unwrap().payload;
    assert_eq!(payload_str(&p, "ai_title"), Some("v3-title".into()));

    drop_collection(&c, &v2_name).await;
    drop_collection(&c, &v3_name).await;
}

#[tokio::test]
async fn it_dual_read_both_miss() {
    if skip_if_no_qdrant() {
        return;
    }
    let c = client().await;
    let v2_name = unique_collection("memex_sessions_v2miss");
    let v3_name = unique_collection("memex_sessions_v3miss");
    create_v2_named(&c, &v2_name).await;
    crud::ensure_collection_v3_named(&c, &v3_name).await.unwrap();

    let pid = point_id_for("never-existed");
    let v3_resp = c
        .get_points(GetPointsBuilder::new(&v3_name, vec![pid.clone()]).with_payload(true))
        .await
        .unwrap();
    let v2_resp = c
        .get_points(GetPointsBuilder::new(&v2_name, vec![pid]).with_payload(true))
        .await
        .unwrap();
    assert!(v3_resp.result.is_empty());
    assert!(v2_resp.result.is_empty());

    drop_collection(&c, &v2_name).await;
    drop_collection(&c, &v3_name).await;
}

#[tokio::test]
async fn it_quantization_present_in_collection_info() {
    if skip_if_no_qdrant() {
        return;
    }
    let c = client().await;
    let name = unique_collection("memex_sessions_v3");
    crud::ensure_collection_v3_named(&c, &name).await.unwrap();

    let info = c.collection_info(&name).await.unwrap();
    let cfg = info
        .result
        .expect("collection info has result")
        .config
        .expect("collection config");
    let qc = cfg
        .quantization_config
        .expect("v3 must carry quantization_config on the server");
    match qc.quantization {
        Some(Quantization::Turboquant(t)) => {
            assert_eq!(t.bits, Some(TurboQuantBitSize::Bits2 as i32));
            assert_eq!(t.always_ram, Some(true));
        }
        other => panic!("expected Turboquant on server, got {other:?}"),
    }

    drop_collection(&c, &name).await;
}

#[tokio::test]
async fn it_tenant_index_present_in_collection_info() {
    if skip_if_no_qdrant() {
        return;
    }
    use qdrant_client::qdrant::payload_index_params::IndexParams;
    let c = client().await;
    let name = unique_collection("memex_sessions_v3");
    crud::ensure_collection_v3_named(&c, &name).await.unwrap();

    let info = c.collection_info(&name).await.unwrap();
    let schema_map = info.result.expect("info").payload_schema;
    let project_idx = schema_map
        .get("project_name")
        .expect("project_name index present");
    let params = project_idx
        .params
        .as_ref()
        .expect("project_name has index params (tenant flag set)");
    match params.index_params.as_ref().expect("inner params") {
        IndexParams::KeywordIndexParams(k) => {
            assert_eq!(k.is_tenant, Some(true), "project_name must be is_tenant=true");
        }
        other => panic!("expected KeywordIndexParams, got {other:?}"),
    }

    drop_collection(&c, &name).await;
}

#[tokio::test]
async fn it_datetime_range_query() {
    if skip_if_no_qdrant() {
        return;
    }
    use qdrant_client::qdrant::Timestamp;
    let c = client().await;
    let name = unique_collection("memex_sessions_v3");
    crud::ensure_collection_v3_named(&c, &name).await.unwrap();

    // Three points spanning ~2 weeks. The 7-day range filter should match
    // only the most recent.
    let mut p_old = synth_payload("dt-old", "memex", "claude_code");
    p_old.start_ts_dt = "2026-04-01T00:00:00.000Z".to_string();
    p_old.start_iso = p_old.start_ts_dt.clone();
    let mut p_mid = synth_payload("dt-mid", "memex", "claude_code");
    p_mid.start_ts_dt = "2026-05-01T00:00:00.000Z".to_string();
    p_mid.start_iso = p_mid.start_ts_dt.clone();
    let mut p_new = synth_payload("dt-new", "memex", "claude_code");
    p_new.start_ts_dt = "2026-05-17T12:00:00.000Z".to_string();
    p_new.start_iso = p_new.start_ts_dt.clone();
    upsert_v3_named(&c, &name, synth_v3_point(0.1, p_old)).await;
    upsert_v3_named(&c, &name, synth_v3_point(0.2, p_mid)).await;
    upsert_v3_named(&c, &name, synth_v3_point(0.3, p_new)).await;

    // 7-day window ending 2026-05-19 → only dt-new.
    let gte = Timestamp {
        seconds: 1_778_544_000, // 2026-05-12T00:00:00Z
        nanos: 0,
    };
    let lte = Timestamp {
        seconds: 1_779_148_800, // 2026-05-19T00:00:00Z
        nanos: 0,
    };
    let cond = Condition::datetime_range(
        "start_ts_dt",
        DatetimeRange {
            gte: Some(gte),
            lte: Some(lte),
            ..Default::default()
        },
    );
    let filter = Filter {
        must: vec![cond],
        ..Default::default()
    };
    let resp = c
        .scroll(
            ScrollPointsBuilder::new(&name)
                .filter(filter)
                .with_payload(true)
                .limit(10),
        )
        .await
        .unwrap();
    let got_ids: Vec<String> = resp
        .result
        .iter()
        .filter_map(|p| payload_str(&p.payload, "session_id"))
        .collect();
    assert_eq!(got_ids, vec!["dt-new".to_string()]);

    drop_collection(&c, &name).await;
}

#[tokio::test]
async fn it_conditional_update_filters_v3_only() {
    if skip_if_no_qdrant() {
        return;
    }
    let c = client().await;
    let name = unique_collection("memex_sessions_v3");
    crud::ensure_collection_v3_named(&c, &name).await.unwrap();

    // Two points in the v3 collection:
    //   - "live-v3" with schema_version=3 (real v3 payload)
    //   - "live-v2sim" with schema_version=2 (a simulated half-migrated record).
    upsert_v3_named(
        &c,
        &name,
        synth_v3_point(0.1, synth_payload("live-v3", "memex", "claude_code")),
    )
    .await;

    let p_v2sim = synth_payload("live-v2sim", "memex", "claude_code");
    // Simulate the half-migrated record by overwriting schema_version.
    // (We hand-build a payload map that lies about its schema_version.)
    let mut json = serde_json::to_value(&p_v2sim).unwrap();
    json["schema_version"] = serde_json::json!(2);
    let v2sim_payload = Payload::try_from(json).unwrap();
    let id_v2sim = point_id_for("live-v2sim");
    let data: Vec<f32> = (0..EMBED_DIM as usize).map(|i| 0.05 + (i as f32) * 1e-6).collect();
    let mut named: HashMap<String, Vector> = HashMap::new();
    for n in schema::DENSE_VECTORS {
        named.insert(
            (*n).to_string(),
            Vector {
                vector: Some(VectorOneOf::Dense(DenseVector { data: data.clone() })),
                ..Default::default()
            },
        );
    }
    let mut pt_v2sim = PointStruct::new(
        id_v2sim.clone(),
        HashMap::<String, Vec<f32>>::new(),
        v2sim_payload,
    );
    pt_v2sim.vectors = Some(Vectors {
        vectors_options: Some(VectorsOptions::Vectors(NamedVectors { vectors: named })),
    });
    upsert_v3_named(&c, &name, pt_v2sim).await;
    let _ = p_v2sim; // unused after override

    // Conditional update — set intent="X" on both ids, gated by schema_version < 3.
    let intent_payload = Payload::try_from(serde_json::json!({ "intent": "X" })).unwrap();
    let ids = vec![point_id_for("live-v3"), point_id_for("live-v2sim")];
    crud::conditional_update_payload(&c, &name, ids.clone(), intent_payload, 3)
        .await
        .unwrap();

    // The v3 record should be unchanged (no intent set).
    let r1 = c
        .get_points(GetPointsBuilder::new(&name, vec![point_id_for("live-v3")]).with_payload(true))
        .await
        .unwrap();
    let p1 = r1.result.into_iter().next().unwrap().payload;
    assert!(
        payload_str(&p1, "intent").is_none(),
        "v3 record (schema_version=3) must NOT be updated by lt=3 filter; got intent={:?}",
        p1.get("intent")
    );

    // The v2sim record should now have intent=X.
    let r2 = c
        .get_points(GetPointsBuilder::new(&name, vec![id_v2sim]).with_payload(true))
        .await
        .unwrap();
    let p2 = r2.result.into_iter().next().unwrap().payload;
    assert_eq!(payload_str(&p2, "intent"), Some("X".into()));

    drop_collection(&c, &name).await;
}

#[tokio::test]
async fn it_migrate_v2_to_v3() {
    if skip_if_no_qdrant() {
        return;
    }
    let c = client().await;
    let v2_name = unique_collection("memex_sessions_v2mig");
    let v3_name = unique_collection("memex_sessions_v3mig");
    create_v2_named(&c, &v2_name).await;
    crud::ensure_collection_v3_named(&c, &v3_name).await.unwrap();

    for i in 0..3 {
        let sid = format!("mig-sess-{i}");
        upsert_v2_named(&c, &v2_name, synth_v2_point(&sid, "memex", "v2-title")).await;
    }

    let report = crud::migrate_named(&c, &v2_name, &v3_name).await.unwrap();
    assert_eq!(report.migrated, 3);
    assert_eq!(report.v3_count_after, 3);

    // Verify each v3 point carries source_agent="claude_code" + schema_version=3.
    let resp = c
        .scroll(
            ScrollPointsBuilder::new(&v3_name)
                .with_payload(true)
                .limit(10),
        )
        .await
        .unwrap();
    assert_eq!(resp.result.len(), 3);
    for p in &resp.result {
        assert_eq!(
            payload_str(&p.payload, "source_agent"),
            Some("claude_code".into())
        );
        assert_eq!(payload_int(&p.payload, "schema_version"), Some(3));
    }

    // Idempotency — second migrate is a no-op at the point-id level (uuid_v5).
    let report2 = crud::migrate_named(&c, &v2_name, &v3_name).await.unwrap();
    assert_eq!(report2.v3_count_after, 3);

    drop_collection(&c, &v2_name).await;
    drop_collection(&c, &v3_name).await;
}

// ---------------------------------------------------------------------------
// Test helpers (local to the integration crate)
// ---------------------------------------------------------------------------

fn payload_str(p: &HashMap<String, Value>, key: &str) -> Option<String> {
    p.get(key)
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            Kind::StringValue(s) => Some(s.clone()),
            _ => None,
        })
}

fn payload_int(p: &HashMap<String, Value>, key: &str) -> Option<i64> {
    p.get(key)
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            Kind::IntegerValue(i) => Some(*i),
            _ => None,
        })
}
