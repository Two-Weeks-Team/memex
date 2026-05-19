//! CRUD primitives for the v3 schema (Phase 3).
//!
//! Responsibilities:
//! - **ensure_collection_v3**: idempotent v3 collection + payload-index creation.
//! - **migrate_v2_to_v3**: paginated scroll of v2, fresh upsert into v3 with the
//!   new payload shape. UUID v5 makes this idempotent at the point-id level.
//! - **dual_get_session_payload / dual_get_points**: v3 first, fall back to v2.
//! - **conditional_update_payload**: `set_payload` gated by a `schema_version < N`
//!   filter so P5's enrich pass only touches un-enriched points.

use std::collections::HashMap;
use std::time::Instant;

use anyhow::{Context, Result};
use qdrant_client::qdrant::{
    point_id::PointIdOptions, points_selector::PointsSelectorOneOf, value::Kind,
    vector::Vector as VectorOneOf, vector_output, vectors::VectorsOptions, vectors_output,
    CreateFieldIndexCollectionBuilder, DenseVector, Filter, GetPointsBuilder, NamedVectors,
    PointId, PointStruct, Range, RetrievedPoint, ScrollPointsBuilder, SetPayloadPointsBuilder,
    UpsertPointsBuilder, Value, Vector, Vectors, VectorsOutput,
};
use qdrant_client::{Payload, Qdrant};

use crate::indexer::{self, COLLECTION as COLLECTION_V2};
use crate::schema::{self, V3Payload, COLLECTION_V3};

const MIGRATE_BATCH_SIZE: u32 = 100;

// ---------------------------------------------------------------------------
// Collection creation
// ---------------------------------------------------------------------------

/// Create the v3 collection + all payload indexes. Safe to call on every
/// startup; no-op if the collection already exists, and best-effort skip on
/// individual `create_field_index` calls (Qdrant returns OK on idempotent
/// retries but a fresh error on the first failure — the inner `let _ = `
/// matches the existing v2 behavior in `indexer::ensure_collection`).
pub async fn ensure_collection_v3(client: &Qdrant) -> Result<()> {
    if client.collection_exists(COLLECTION_V3).await? {
        return Ok(());
    }
    client
        .create_collection(schema::build_v3_create_collection())
        .await
        .with_context(|| format!("creating {COLLECTION_V3}"))?;

    for (field, ftype, params) in schema::v3_payload_indexes() {
        let mut b = CreateFieldIndexCollectionBuilder::new(COLLECTION_V3, field, ftype);
        if let Some(p) = params {
            // SPEC NOTE (P3, KC-03/KC-04): builder.field_index_params expects
            // the inner `payload_index_params::IndexParams` oneof, not the
            // wrapper `PayloadIndexParams`. We unwrap one level.
            if let Some(inner) = p.index_params {
                b = b.field_index_params(inner);
            }
        }
        // Best-effort. On a fresh collection these should all succeed; the
        // skip is for the (rare) parallel-startup case where another tab
        // already created the index.
        let _ = client.create_field_index(b.build()).await;
    }
    Ok(())
}

/// Variant that operates on any caller-provided collection name. Used by
/// integration tests so they don't trample the real v3 collection.
pub async fn ensure_collection_v3_named(client: &Qdrant, name: &str) -> Result<()> {
    if client.collection_exists(name).await? {
        return Ok(());
    }
    let mut builder = schema::build_v3_create_collection();
    builder = builder.collection_name(name.to_string());
    client
        .create_collection(builder)
        .await
        .with_context(|| format!("creating {name}"))?;
    for (field, ftype, params) in schema::v3_payload_indexes() {
        let mut b = CreateFieldIndexCollectionBuilder::new(name, field, ftype);
        if let Some(p) = params {
            if let Some(inner) = p.index_params {
                b = b.field_index_params(inner);
            }
        }
        let _ = client.create_field_index(b.build()).await;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dual read
// ---------------------------------------------------------------------------

/// V3-first, V2-fallback session payload fetch. Returns `Ok(None)` only when
/// neither collection has the point.
pub async fn dual_get_session_payload(
    client: &Qdrant,
    session_id: &str,
) -> Result<Option<HashMap<String, Value>>> {
    let pid = PointId {
        point_id_options: Some(PointIdOptions::Uuid(indexer::point_id(session_id))),
    };

    // 1) Try v3 (only if it exists, so a fresh DB doesn't error on lookup).
    // CORRECTNESS FIX (Codex/Gemini review on PR #3, crud.rs:104+): the
    // previous `unwrap_or(false)` collapsed transient Qdrant errors
    // (connectivity, auth, server panic) to "collection does not exist",
    // which is silently the wrong answer — we'd return partial / `None`
    // data instead of surfacing the outage. Propagate the error with `?`.
    if client.collection_exists(COLLECTION_V3).await? {
        let res = client
            .get_points(GetPointsBuilder::new(COLLECTION_V3, vec![pid.clone()]).with_payload(true))
            .await?;
        if let Some(p) = res.result.into_iter().next() {
            return Ok(Some(p.payload));
        }
    }
    // 2) Fall back to v2 (propagate errors per the same rationale above).
    if client.collection_exists(COLLECTION_V2).await? {
        let res = client
            .get_points(GetPointsBuilder::new(COLLECTION_V2, vec![pid]).with_payload(true))
            .await?;
        if let Some(p) = res.result.into_iter().next() {
            return Ok(Some(p.payload));
        }
    }
    Ok(None)
}

/// Same as `dual_get_session_payload` but for batch point-id lookups (used by
/// the topology view). Returns whatever the union of v3 + v2 yields, deduped
/// by point id (v3 wins on collision).
pub async fn dual_get_points(
    client: &Qdrant,
    point_ids: Vec<PointId>,
) -> Result<Vec<RetrievedPoint>> {
    if point_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_id: HashMap<String, RetrievedPoint> = HashMap::new();
    let mut missing: Vec<PointId> = point_ids.clone();

    // CORRECTNESS FIX (Codex/Gemini review on PR #3, crud.rs:104+): the
    // previous `unwrap_or(false)` collapsed transient Qdrant errors
    // (connectivity, auth, server panic) to "collection does not exist",
    // which is silently the wrong answer — we'd return partial / `None`
    // data instead of surfacing the outage. Propagate the error with `?`.
    if client.collection_exists(COLLECTION_V3).await? {
        let v3 = client
            .get_points(
                GetPointsBuilder::new(COLLECTION_V3, point_ids.clone()).with_payload(true),
            )
            .await?;
        for p in v3.result {
            if let Some(id_s) = point_id_opt_to_string(&p.id) {
                by_id.insert(id_s, p);
            }
        }
        missing.retain(|pid| {
            point_id_to_string(pid)
                .map(|s| !by_id.contains_key(&s))
                .unwrap_or(true)
        });
    }

    if !missing.is_empty()
        && client.collection_exists(COLLECTION_V2).await?
    {
        let v2 = client
            .get_points(GetPointsBuilder::new(COLLECTION_V2, missing).with_payload(true))
            .await?;
        for p in v2.result {
            if let Some(id_s) = point_id_opt_to_string(&p.id) {
                by_id.entry(id_s).or_insert(p);
            }
        }
    }

    Ok(by_id.into_values().collect())
}

fn point_id_opt_to_string(id: &Option<PointId>) -> Option<String> {
    point_id_to_string(id.as_ref()?)
}

fn point_id_to_string(id: &PointId) -> Option<String> {
    match id.point_id_options.as_ref()? {
        PointIdOptions::Uuid(u) => Some(u.clone()),
        PointIdOptions::Num(n) => Some(n.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Conditional update (KG-04)
// ---------------------------------------------------------------------------

/// Set payload on points selected by id AND filtered by `schema_version < limit`.
///
/// SPEC NOTE (P3, KG-04): The plan spec mentions `UpdateMode::Update`, but the
/// 1.18 `UpdateMode` enum only has `Upsert`/`InsertOnly`/`UpdateOnly` — those
/// are upsert-side modes (whether to insert/update points), not payload-set
/// modes. The actual "only-set-if-condition" semantics are achieved by
/// combining a `PointsSelector` with both an id-filter AND a payload-condition
/// filter. We model that here: the selector intersects (point in `ids`) with
/// (schema_version < limit). If the point is already at schema_version=N (e.g.
/// after P5 enrich), it's silently skipped on subsequent calls — making this
/// safe to invoke repeatedly.
pub async fn conditional_update_payload(
    client: &Qdrant,
    collection: &str,
    point_ids: Vec<PointId>,
    payload: Payload,
    schema_version_lt: u32,
) -> Result<()> {
    let filter = build_conditional_filter(&point_ids, schema_version_lt);
    let req = SetPayloadPointsBuilder::new(collection, payload)
        .points_selector(PointsSelectorOneOf::Filter(filter))
        .wait(true);
    client.set_payload(req).await?;
    Ok(())
}

/// Internal — used by tests to inspect the produced filter without hitting
/// the server.
pub fn build_conditional_filter(point_ids: &[PointId], schema_version_lt: u32) -> Filter {
    use qdrant_client::qdrant::{Condition, HasIdCondition};

    let has_id_cond = Condition {
        condition_one_of: Some(qdrant_client::qdrant::condition::ConditionOneOf::HasId(
            HasIdCondition {
                has_id: point_ids.to_vec(),
            },
        )),
    };
    let lt_cond = Condition::range(
        "schema_version",
        Range {
            lt: Some(schema_version_lt as f64),
            ..Default::default()
        },
    );
    Filter {
        must: vec![has_id_cond, lt_cond],
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// V2 → V3 migration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct MigrationReport {
    pub v2_count: u64,
    pub v3_count_before: u64,
    pub v3_count_after: u64,
    pub migrated: u64,
    pub elapsed_ms: u128,
}

/// Walk every point in v2 (paginated scroll) and re-upsert into v3 with the
/// extended payload. UUID v5 on `session_id` ensures the same point-id on both
/// sides, so re-running this is a no-op once v3 catches up.
///
/// Vectors are NOT recomputed — we read them out of v2 (`with_vectors=true`)
/// and write them back into v3 unchanged. The new multivector + sparse vectors
/// stay empty until P4 wires them up; v3 upserts that omit a named vector are
/// accepted by Qdrant (the vector slot just stays empty for that point).
pub async fn migrate_v2_to_v3(client: &Qdrant) -> Result<MigrationReport> {
    migrate_named(client, COLLECTION_V2, COLLECTION_V3).await
}

/// Auto-migration wrapper used by `AppState::qdrant()` on startup. Returns:
/// - `Ok(None)` when no migration is needed (v2 empty OR v3 already has data
///   ≥ v2 — we treat the user as already migrated)
/// - `Ok(Some(report))` when a migration ran
/// - `Err(_)` on transient Qdrant failure (caller logs + swallows)
///
/// Conservative on purpose: we never re-migrate a v3 that already has any
/// points, since that would (a) be a no-op (idempotent upsert by point_id)
/// and (b) burn user-visible time on every restart.
pub async fn migrate_v2_to_v3_if_needed(client: &Qdrant) -> Result<Option<MigrationReport>> {
    let v2_exists = client
        .collection_exists(COLLECTION_V2)
        .await
        .with_context(|| format!("collection_exists({COLLECTION_V2}) probe"))?;
    if !v2_exists {
        return Ok(None);
    }
    let v2_count = count_points(client, COLLECTION_V2).await.unwrap_or(0);
    if v2_count == 0 {
        return Ok(None);
    }
    let v3_count = count_points(client, COLLECTION_V3).await.unwrap_or(0);
    if v3_count >= v2_count {
        // v3 already covers (or exceeds) v2 — typical post-`scan --index` state.
        return Ok(None);
    }
    let report = migrate_v2_to_v3(client).await?;
    Ok(Some(report))
}

/// Test hook — migrate between arbitrary named collections.
pub async fn migrate_named(client: &Qdrant, from: &str, to: &str) -> Result<MigrationReport> {
    let start = Instant::now();
    let v2_count = count_points(client, from).await.unwrap_or(0);
    let v3_count_before = count_points(client, to).await.unwrap_or(0);

    let mut migrated = 0u64;
    let mut offset: Option<PointId> = None;
    loop {
        let mut b = ScrollPointsBuilder::new(from)
            .with_payload(true)
            .with_vectors(true)
            .limit(MIGRATE_BATCH_SIZE);
        if let Some(off) = offset.clone() {
            b = b.offset(off);
        }
        let resp = client
            .scroll(b)
            .await
            .with_context(|| format!("scroll {from}"))?;

        if resp.result.is_empty() {
            break;
        }

        let mut to_upsert: Vec<PointStruct> = Vec::with_capacity(resp.result.len());
        for rec in &resp.result {
            // Lift v2 payload → v3 typed payload.
            let v3p = match V3Payload::from_v2_payload(&rec.payload) {
                Ok(p) => p,
                Err(_) => continue, // skip malformed
            };
            let payload = v3p.to_qdrant_payload();
            // Carry the v2 dense vectors across unchanged. `VectorsOutput` is
            // the response-side type; we convert each named dense vector into
            // the request-side `Vector::Dense` for the v3 upsert.
            let dense_map = vectors_output_to_named_dense(rec.vectors.as_ref());
            let id = rec.id.clone().expect("scrolled point has id");
            let mut pt = PointStruct::new(
                id,
                HashMap::<String, Vec<f32>>::new(),
                payload,
            );
            if !dense_map.is_empty() {
                pt.vectors = Some(Vectors {
                    vectors_options: Some(VectorsOptions::Vectors(NamedVectors {
                        vectors: dense_map,
                    })),
                });
            }
            to_upsert.push(pt);
        }

        if !to_upsert.is_empty() {
            let n = to_upsert.len() as u64;
            client
                .upsert_points(UpsertPointsBuilder::new(to, to_upsert).wait(true))
                .await
                .with_context(|| format!("upsert into {to}"))?;
            migrated += n;
        }

        offset = resp.next_page_offset;
        if offset.is_none() {
            break;
        }
    }

    let v3_count_after = count_points(client, to).await.unwrap_or(v3_count_before);
    Ok(MigrationReport {
        v2_count,
        v3_count_before,
        v3_count_after,
        migrated,
        elapsed_ms: start.elapsed().as_millis(),
    })
}

/// Convert a `VectorsOutput` (scroll/get response shape) into the named-dense
/// map used by request-side `Vectors`. Sparse + multivector entries are
/// dropped — v2 only has dense, and the v3 sparse/multivec slots are still
/// empty (P4 will fill them). The map is keyed by vector name.
fn vectors_output_to_named_dense(vo: Option<&VectorsOutput>) -> HashMap<String, Vector> {
    use vectors_output::VectorsOptions as OutOpts;
    let Some(vo) = vo else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    match vo.vectors_options.as_ref() {
        Some(OutOpts::Vectors(named)) => {
            for (name, vout) in &named.vectors {
                // Dense path only — pull data either from the (deprecated) data
                // field or from the new `vector` oneof's Dense variant.
                #[allow(deprecated)]
                let dense_data: Vec<f32> = match vout.vector.as_ref() {
                    Some(vector_output::Vector::Dense(d)) => d.data.clone(),
                    _ => vout.data.clone(),
                };
                if dense_data.is_empty() {
                    continue;
                }
                out.insert(
                    name.clone(),
                    Vector {
                        vector: Some(VectorOneOf::Dense(DenseVector { data: dense_data })),
                        ..Default::default()
                    },
                );
            }
        }
        Some(OutOpts::Vector(_)) | None => {}
    }
    out
}

async fn count_points(client: &Qdrant, name: &str) -> Result<u64> {
    if !client.collection_exists(name).await? {
        return Ok(0);
    }
    let info = client.collection_info(name).await?;
    Ok(info
        .result
        .and_then(|i| i.points_count)
        .unwrap_or(0))
}

// ---------------------------------------------------------------------------
// Helpers re-exposed for indexer wiring
// ---------------------------------------------------------------------------

/// Convenience: deterministic `PointId` from a session id.
pub fn point_id_for(session_id: &str) -> PointId {
    PointId {
        point_id_options: Some(PointIdOptions::Uuid(indexer::point_id(session_id))),
    }
}

/// Read `schema_version` (int) from a payload map. Used by the dual-read
/// merge logic in tests.
pub fn payload_schema_version(p: &HashMap<String, Value>) -> Option<i64> {
    p.get("schema_version")
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            Kind::IntegerValue(i) => Some(*i),
            _ => None,
        })
}

// ---------------------------------------------------------------------------
// Tests (§4 from tests.md)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use qdrant_client::qdrant::condition::ConditionOneOf;

    #[test]
    fn t_conditional_filter_has_id_and_range() {
        // AC-3.5.1 / AC-3.5.2 — the produced filter must have both:
        //   - HasId(point_ids)
        //   - Range(schema_version, lt=3)
        let ids = vec![
            PointId {
                point_id_options: Some(PointIdOptions::Uuid(
                    "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
                )),
            },
            PointId {
                point_id_options: Some(PointIdOptions::Uuid(
                    "11111111-2222-3333-4444-555555555555".into(),
                )),
            },
        ];
        let filter = build_conditional_filter(&ids, 3);
        assert_eq!(filter.must.len(), 2, "must has 2 clauses");
        let mut saw_has_id = false;
        let mut saw_range = false;
        for cond in &filter.must {
            match &cond.condition_one_of {
                Some(ConditionOneOf::HasId(h)) => {
                    assert_eq!(h.has_id.len(), 2);
                    saw_has_id = true;
                }
                Some(ConditionOneOf::Field(f)) => {
                    assert_eq!(f.key, "schema_version");
                    let r = f.range.expect("range set on schema_version cond");
                    assert_eq!(r.lt, Some(3.0));
                    saw_range = true;
                }
                other => panic!("unexpected condition variant: {other:?}"),
            }
        }
        assert!(saw_has_id, "filter must include HasId");
        assert!(saw_range, "filter must include Range");
    }

    #[test]
    fn t_conditional_filter_lt_threshold_floats() {
        // Sanity — different thresholds round-trip into the lt field.
        let f = build_conditional_filter(&[], 5);
        let range = f
            .must
            .iter()
            .find_map(|c| match &c.condition_one_of {
                Some(ConditionOneOf::Field(fc)) => fc.range,
                _ => None,
            })
            .expect("range cond present");
        assert_eq!(range.lt, Some(5.0));
    }

    #[test]
    fn t_dual_read_helpers_compose() {
        // No-live-Qdrant check: the helper that extracts schema_version
        // distinguishes int from string and missing.
        let mut p = HashMap::new();
        p.insert(
            "schema_version".to_string(),
            Value {
                kind: Some(Kind::IntegerValue(3)),
            },
        );
        assert_eq!(payload_schema_version(&p), Some(3));

        let mut p2 = HashMap::new();
        p2.insert(
            "schema_version".to_string(),
            Value {
                kind: Some(Kind::StringValue("3".into())),
            },
        );
        assert_eq!(payload_schema_version(&p2), None, "string is not int");

        let p3 = HashMap::new();
        assert_eq!(payload_schema_version(&p3), None);
    }

    #[test]
    fn t_point_id_for_session_is_uuid_v5() {
        // Idempotency invariant — same session id always maps to same point id.
        let a = point_id_for("abc-123");
        let b = point_id_for("abc-123");
        assert_eq!(a.point_id_options, b.point_id_options);
        // Different ids → different point ids.
        let c = point_id_for("xyz-789");
        assert_ne!(a.point_id_options, c.point_id_options);
    }
}
