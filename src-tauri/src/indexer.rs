//! Qdrant indexing for Memex.
//!
//! One point per session, 5 named dense vectors (BGE-small-en-v1.5, 384-d):
//!
//! - `content` — full conversation transcript text (user+assistant prose only)
//! - `tool`    — tool call descriptors (`<ToolName>: <key-input>` lines)
//! - `path`    — file paths mentioned anywhere (tool inputs, text references)
//! - `error`   — tool_result text where `is_error=true` + "Error:" phrases
//! - `code`    — fenced code blocks + Edit/Write contents
//!
//! BGE-small is used for all 5 vectors in this MVP. Plan §2 calls for BM42
//! sparse on `path` and ColBERT multi-vector on `content` — those are deferred
//! to Phase 3+ once the search loop is wired end-to-end.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use once_cell::sync::Lazy;
use qdrant_client::{
    qdrant::{
        point_id::PointIdOptions, vectors_config, ContextInputBuilder, ContextInputPair,
        CreateCollectionBuilder, CreateFieldIndexCollectionBuilder, Distance, FieldType,
        Filter, PointId, PointStruct, Query,
        QueryPointsBuilder, SearchMatrixPointsBuilder, UpsertPointsBuilder, VectorInput,
        VectorParamsBuilder, VectorParamsMap, VectorsConfig,
    },
    Payload, Qdrant,
};
use regex::Regex;
use serde_json::json;

use crate::parser::{Session, ToolCall, TurnRole};

pub const COLLECTION: &str = "memex_sessions";
pub const EMBED_DIM: u64 = 384;
pub const VECTORS: &[&str] = &["content", "tool", "path", "error", "code"];

const MAX_CHARS_PER_VECTOR: usize = 6_000;
const EMBED_BATCH: usize = 32;

static CODE_FENCE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"```[\w+-]*\n([\s\S]*?)```").unwrap());

// P5 KG-01 — Topology insights memo cache. One process-wide instance so
// repeated `topology` calls within a session benefit from the warm cache.
static INSIGHTS_CACHE: Lazy<crate::insights_cache::InsightsCache> =
    Lazy::new(crate::insights_cache::InsightsCache::new);

// P5 KG-02 — Predict pivot-parse LRU. Same process-wide singleton pattern;
// memoizes `parser::parse_session` per (path, mtime).
static PREDICT_PARSE_CACHE: Lazy<crate::parse_cache::ParseLruCache> =
    Lazy::new(crate::parse_cache::ParseLruCache::new);

/// Wraps a fastembed `TextEmbedding` (BGE-small-en-v1.5). The model needs
/// `&mut self` to embed (internal ONNX session state), so we serialize access
/// via a `Mutex` and let callers use `&Embedder`.
pub struct Embedder {
    inner: Mutex<TextEmbedding>,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let cache_dir = default_fastembed_cache_dir();
        // Best-effort — fastembed will surface a clearer error if creation fails.
        std::fs::create_dir_all(&cache_dir).ok();
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_show_download_progress(true)
                .with_cache_dir(cache_dir.clone()),
        )
        .with_context(|| {
            format!(
                "loading BGE-small-en-v1.5 fastembed model (cache_dir={})",
                cache_dir.display()
            )
        })?;
        Ok(Self {
            inner: Mutex::new(model),
        })
    }

    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        // (impl below)
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity(texts.len());
        let mut model = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("embedder mutex poisoned: {e}"))?;
        for chunk in texts.chunks(EMBED_BATCH) {
            let chunk_refs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
            let batch = model
                .embed(chunk_refs, None)
                .context("fastembed embed() failed")?;
            out.extend(batch);
        }
        Ok(out)
    }
}

/// Resolve a writable cache dir for the fastembed ONNX model. The .app
/// bundle on macOS launches with CWD=`/` (read-only), so the default
/// `.fastembed_cache/` placement next to CWD fails with EROFS. We pick
/// the platform-canonical cache dir and create it if missing.
fn default_fastembed_cache_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    // Honor explicit override first (useful for tests + sandboxes).
    if let Ok(p) = std::env::var("MEMEX_FASTEMBED_CACHE_DIR") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    #[cfg(target_os = "macos")]
    {
        return PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("dev.sgwannabe.memex")
            .join("fastembed");
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
            return PathBuf::from(xdg).join("memex").join("fastembed");
        }
        PathBuf::from(home).join(".cache").join("memex").join("fastembed")
    }
}

/// Connect to local Qdrant (default `http://localhost:6334` gRPC).
pub async fn connect() -> Result<Qdrant> {
    let url = std::env::var("MEMEX_QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".into());
    Qdrant::from_url(&url)
        .build()
        .with_context(|| format!("connecting to qdrant at {url}"))
}

/// Create the collection + payload indexes if not present (idempotent).
pub async fn ensure_collection(client: &Qdrant) -> Result<()> {
    if client.collection_exists(COLLECTION).await? {
        return Ok(());
    }
    let mut params: HashMap<String, _> = HashMap::new();
    for name in VECTORS {
        params.insert(
            (*name).to_string(),
            VectorParamsBuilder::new(EMBED_DIM, Distance::Cosine).build(),
        );
    }
    let vectors_cfg: VectorsConfig =
        vectors_config::Config::ParamsMap(VectorParamsMap { map: params }).into();

    client
        .create_collection(
            CreateCollectionBuilder::new(COLLECTION).vectors_config(vectors_cfg),
        )
        .await?;

    for (field, ftype) in [
        ("project_name", FieldType::Keyword),
        ("project_path", FieldType::Keyword),
        ("git_branch", FieldType::Keyword),
        ("ai_title", FieldType::Text),
        ("start_ts", FieldType::Integer),
        ("has_errors", FieldType::Bool),
    ] {
        // Best-effort: indexes already exist on re-run.
        let _ = client
            .create_field_index(
                CreateFieldIndexCollectionBuilder::new(COLLECTION, field, ftype).build(),
            )
            .await;
    }
    Ok(())
}

pub fn session_extracts(session: &Session) -> [(String, String); 5] {
    let content = build_content(session);
    let tool = build_tool(session);
    let path = build_path(session);
    let error = build_error(session);
    let code = build_code(session);
    [
        ("content".into(), cap(&content)),
        ("tool".into(), cap(&tool)),
        ("path".into(), cap(&path)),
        ("error".into(), cap(&error)),
        ("code".into(), cap(&code)),
    ]
}

fn cap(s: &str) -> String {
    if s.chars().count() <= MAX_CHARS_PER_VECTOR {
        s.to_string()
    } else {
        s.chars().take(MAX_CHARS_PER_VECTOR).collect()
    }
}

fn build_content(s: &Session) -> String {
    let mut buf = String::new();
    if let Some(t) = &s.ai_title {
        buf.push_str("title: ");
        buf.push_str(t);
        buf.push('\n');
    }
    for turn in &s.turns {
        if turn.text.is_empty() {
            continue;
        }
        match turn.role {
            TurnRole::User => buf.push_str("U: "),
            TurnRole::Assistant => buf.push_str("A: "),
            TurnRole::System => continue,
        }
        buf.push_str(&turn.text);
        buf.push('\n');
    }
    if buf.is_empty() {
        buf.push_str(s.project_name.as_deref().unwrap_or("session"));
    }
    buf
}

fn build_tool(s: &Session) -> String {
    let mut lines = Vec::new();
    for turn in &s.turns {
        for tc in &turn.tool_calls {
            lines.push(format!("{}: {}", tc.name, tool_input_snippet(tc)));
        }
    }
    if lines.is_empty() {
        lines.push("(no tool calls)".to_string());
    }
    lines.join("\n")
}

fn tool_input_snippet(tc: &ToolCall) -> String {
    let preview_keys = [
        "command",
        "file_path",
        "url",
        "query",
        "pattern",
        "path",
        "description",
    ];
    for k in preview_keys {
        if let Some(v) = tc.input.get(k).and_then(|x| x.as_str()) {
            if !v.is_empty() {
                return v.chars().take(160).collect();
            }
        }
    }
    let s = serde_json::to_string(&tc.input).unwrap_or_default();
    s.chars().take(160).collect()
}

fn build_path(s: &Session) -> String {
    use std::collections::BTreeSet;
    let mut paths: BTreeSet<String> = BTreeSet::new();
    if let Some(p) = &s.project_path {
        paths.insert(p.clone());
    }
    for turn in &s.turns {
        for tc in &turn.tool_calls {
            for k in ["file_path", "path", "notebook_path"] {
                if let Some(p) = tc.input.get(k).and_then(|x| x.as_str()) {
                    if !p.is_empty() {
                        paths.insert(p.to_string());
                    }
                }
            }
            if let Some(url) = tc.input.get("url").and_then(|x| x.as_str()) {
                paths.insert(url.to_string());
            }
        }
    }
    if paths.is_empty() {
        return s
            .project_path
            .clone()
            .unwrap_or_else(|| "(no paths)".into());
    }
    paths.into_iter().collect::<Vec<_>>().join("\n")
}

fn build_error(s: &Session) -> String {
    let mut chunks = Vec::new();
    for turn in &s.turns {
        for r in &turn.tool_results {
            if r.is_error {
                chunks.push(r.content.chars().take(800).collect::<String>());
            }
        }
        if matches!(turn.role, TurnRole::Assistant) {
            for line in turn.text.lines() {
                let lower = line.to_ascii_lowercase();
                if lower.contains("error:")
                    || lower.contains("failed")
                    || lower.contains("traceback")
                    || lower.contains("panic")
                    || lower.contains("exception")
                {
                    chunks.push(line.trim().to_string());
                }
            }
        }
    }
    if chunks.is_empty() {
        chunks.push("(no errors)".to_string());
    }
    chunks.join("\n")
}

fn build_code(s: &Session) -> String {
    let mut blobs = Vec::new();
    for turn in &s.turns {
        for cap in CODE_FENCE.captures_iter(&turn.text) {
            if let Some(m) = cap.get(1) {
                blobs.push(m.as_str().to_string());
            }
        }
        for tc in &turn.tool_calls {
            for k in ["new_string", "content"] {
                if let Some(v) = tc.input.get(k).and_then(|x| x.as_str()) {
                    if !v.is_empty() {
                        blobs.push(v.chars().take(800).collect());
                    }
                }
            }
        }
    }
    if blobs.is_empty() {
        blobs.push("(no code)".to_string());
    }
    blobs.join("\n---\n")
}

/// Deterministic point ID derived from `session_id` so reindex is idempotent.
pub fn point_id(session_id: &str) -> String {
    let ns = uuid::Uuid::NAMESPACE_DNS;
    uuid::Uuid::new_v5(&ns, session_id.as_bytes()).to_string()
}

fn session_payload(s: &Session) -> Payload {
    let mut tool_count = 0usize;
    let mut has_errors = false;
    for turn in &s.turns {
        tool_count += turn.tool_calls.len();
        if turn.tool_results.iter().any(|r| r.is_error) {
            has_errors = true;
        }
    }
    let payload = json!({
        "session_id": s.session_id,
        "source_path": s.source_path.to_string_lossy(),
        "project_name": s.project_name.as_deref().unwrap_or(""),
        "project_path": s.project_path.as_deref().unwrap_or(""),
        "git_branch": s.git_branch.as_deref().unwrap_or(""),
        "claude_version": s.claude_version.as_deref().unwrap_or(""),
        "ai_title": s.ai_title.as_deref().unwrap_or(""),
        "start_iso": s.start_time.map(|t| t.to_rfc3339()).unwrap_or_default(),
        "end_iso": s.end_time.map(|t| t.to_rfc3339()).unwrap_or_default(),
        "start_ts": s.start_time.map(|t| t.timestamp()).unwrap_or(0),
        "end_ts": s.end_time.map(|t| t.timestamp()).unwrap_or(0),
        "user_turns": s.event_counts.user,
        "assistant_turns": s.event_counts.assistant,
        "tool_count": tool_count,
        "has_errors": has_errors,
    });
    Payload::try_from(payload).expect("payload conversion")
}

/// V3-shaped payload for a parsed Session. Adds:
/// - `schema_version: 3`
/// - `start_ts_dt` / `end_ts_dt` (RFC 3339 strings — used by the datetime index)
/// - `source_agent` (KH-01)
/// - reserved enrich fields (`intent`, `entities`, `outcome`, `arc`, `topic` —
///   all null/empty until P5 enrich.rs fills them)
///
/// The v2-shaped numeric `start_ts` / `end_ts` are dropped on v3 — the datetime
/// index does the heavy lifting. Other fields mirror v2.
fn session_payload_v3(s: &Session) -> Payload {
    let mut tool_count = 0i64;
    let mut has_errors = false;
    for turn in &s.turns {
        tool_count += turn.tool_calls.len() as i64;
        if turn.tool_results.iter().any(|r| r.is_error) {
            has_errors = true;
        }
    }
    let source_path = s.source_path.to_string_lossy().into_owned();
    let start_iso = s.start_time.map(|t| t.to_rfc3339()).unwrap_or_default();
    let end_iso = s.end_time.map(|t| t.to_rfc3339()).unwrap_or_default();
    let mut v3 = crate::schema::V3Payload::from_session_fields(
        s.session_id.clone(),
        source_path,
        s.project_name.clone().unwrap_or_default(),
        s.project_path.clone().unwrap_or_default(),
        s.git_branch.clone().unwrap_or_default(),
        s.ai_title.clone().unwrap_or_default(),
        s.claude_version.clone().unwrap_or_default(),
        start_iso,
        end_iso,
        s.event_counts.user as i64,
        s.event_counts.assistant as i64,
        tool_count,
        has_errors,
    );
    // P5 — populate the 5 enrich-stage fields heuristically (LLM-free).
    // `enrich` is pure and deterministic; safe to call once per session here
    // so the upsert carries the labels without a separate update round-trip.
    let out = crate::enrich::enrich(s, &s.turns);
    v3.intent = Some(out.intent);
    v3.entities = out.entities;
    v3.outcome = Some(out.outcome);
    v3.arc = Some(out.arc);
    v3.topic = Some(out.topic);
    v3.to_qdrant_payload()
}

pub fn build_point(session: &Session, vectors_by_name: Vec<(String, Vec<f32>)>) -> PointStruct {
    let id = point_id(&session.session_id);
    let payload = session_payload(session);
    let vec_map: HashMap<String, Vec<f32>> = vectors_by_name.into_iter().collect();
    PointStruct::new(id, vec_map, payload)
}

/// Build a v3 point with the new payload shape. Vector data is identical to v2
/// (we don't re-embed) — only the payload differs.
pub fn build_point_v3(session: &Session, vectors_by_name: Vec<(String, Vec<f32>)>) -> PointStruct {
    let id = point_id(&session.session_id);
    let payload = session_payload_v3(session);
    let vec_map: HashMap<String, Vec<f32>> = vectors_by_name.into_iter().collect();
    PointStruct::new(id, vec_map, payload)
}

/// **Write path** — upserts only into the v3 collection (KG-03 dual-write rule:
/// v2 is frozen as read-only fallback).
///
/// KB-01 — in addition to the 5 dense vectors, we now also write a
/// `content_late` multivector built from sliding-window token-level chunks of
/// the content text (BGE-small embed per chunk). This is the second-half of
/// the late-interaction MaxSim path; queries are wired in `lens_search`.
pub async fn index_session(
    client: &Qdrant,
    embedder: &Embedder,
    session: &Session,
) -> Result<()> {
    let extracts = session_extracts(session);
    let texts: Vec<String> = extracts.iter().map(|(_, t)| t.clone()).collect();
    let vectors = embedder.embed(texts)?;
    let named_dense: Vec<(String, Vec<f32>)> = extracts
        .iter()
        .map(|(k, _)| k.clone())
        .zip(vectors.into_iter())
        .collect();

    // KB-01 — late-interaction chunks. We use the *content* text (first
    // extract) as the basis because that's what the multivector slot is
    // semantically tied to (`MULTIVECTOR_NAME = "content_late"`).
    let content_text = extracts
        .iter()
        .find(|(k, _)| k == "content")
        .map(|(_, t)| t.clone())
        .unwrap_or_default();
    let chunk_vectors: Vec<Vec<f32>> =
        crate::embed_late::embed_token_level(embedder, &content_text)?;

    let point = build_point_v3_with_multivec(session, named_dense, chunk_vectors);
    client
        .upsert_points(
            UpsertPointsBuilder::new(crate::schema::COLLECTION_V3, vec![point]).wait(true),
        )
        .await?;
    Ok(())
}

/// Build a v3 point with named dense vectors + optional `content_late`
/// multivector. When `multivec_chunks` is empty the multivector slot is
/// simply omitted (Qdrant 1.18 accepts a partial upsert against the named
/// vector schema).
pub fn build_point_v3_with_multivec(
    session: &Session,
    dense: Vec<(String, Vec<f32>)>,
    multivec_chunks: Vec<Vec<f32>>,
) -> PointStruct {
    use qdrant_client::qdrant::{
        vector, vectors::VectorsOptions, DenseVector, MultiDenseVector, NamedVectors,
        Vector as ProtoVector, Vectors,
    };

    let id = point_id_proto(&session.session_id);
    let payload = session_payload_v3(session);
    let mut named: HashMap<String, ProtoVector> = HashMap::new();
    for (k, v) in dense {
        named.insert(
            k,
            ProtoVector {
                vector: Some(vector::Vector::Dense(DenseVector { data: v })),
                ..Default::default()
            },
        );
    }
    if !multivec_chunks.is_empty() {
        let mv = MultiDenseVector::from(multivec_chunks);
        named.insert(
            crate::schema::MULTIVECTOR_NAME.to_string(),
            ProtoVector {
                vector: Some(vector::Vector::MultiDense(mv)),
                ..Default::default()
            },
        );
    }
    PointStruct {
        id: Some(id),
        payload: payload.into(),
        vectors: Some(Vectors {
            vectors_options: Some(VectorsOptions::Vectors(NamedVectors { vectors: named })),
        }),
    }
}

/// Helper — returns a `PointId` from a session id using the same uuid_v5
/// derivation as `point_id`.
fn point_id_proto(session_id: &str) -> PointId {
    PointId {
        point_id_options: Some(qdrant_client::qdrant::point_id::PointIdOptions::Uuid(
            point_id(session_id),
        )),
    }
}

/// Result of a bulk indexing pass — distinguishes "actually indexed" from
/// "silently skipped because of duplicate sessionId" so the caller can be
/// honest about coverage.
#[derive(Debug, Clone, Copy)]
pub struct BulkIndexReport {
    pub indexed: usize,
    pub duplicates_skipped: usize,
    pub errors: usize,
}

pub async fn bulk_index(
    client: &Qdrant,
    embedder: &Embedder,
    sessions: &[Session],
) -> Result<BulkIndexReport> {
    // Keep the legacy `&Embedder` signature compatible by delegating to the
    // Arc-based path. Callers that already have `Arc<Embedder>` (every
    // commands.rs entry point does) should prefer `bulk_index_arc` to skip
    // the extra Arc allocation. For CLI test paths that hold an owned
    // `Embedder` we still need a way to get an Arc — `Arc::new` on the value
    // requires ownership, which the borrow doesn't carry. We can't fix that
    // without breaking the legacy API, so we synthesize a per-call Arc via
    // a no-clone proxy: just call the legacy per-session loop.
    bulk_index_legacy(client, embedder, sessions).await
}

/// New (P5 KC-05) Arc-based entry point. Performs cross-session batched
/// embedding with `embed_pool` + `Semaphore` cap.
pub async fn bulk_index_arc(
    client: &Qdrant,
    embedder: std::sync::Arc<Embedder>,
    sessions: &[Session],
) -> Result<BulkIndexReport> {
    use indicatif::{ProgressBar, ProgressStyle};
    let pb = ProgressBar::new(sessions.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{wide_bar} {pos}/{len} ({eta}) {msg}")
            .unwrap()
            .progress_chars("=> "),
    );

    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut report = BulkIndexReport {
        indexed: 0,
        duplicates_skipped: 0,
        errors: 0,
    };

    let pool = crate::embed_pool::EmbedPool::new(embedder.clone());
    let mut window: Vec<&Session> = Vec::with_capacity(crate::embed_pool::CROSS_SESSION_BATCH);

    for s in sessions {
        let label = s
            .project_name
            .clone()
            .unwrap_or_else(|| s.session_id.clone());
        pb.set_message(label);
        if !seen_ids.insert(s.session_id.clone()) {
            pb.println(format!(
                "  ⊘ duplicate sessionId={} ({}) — kept first occurrence",
                &s.session_id[..8.min(s.session_id.len())],
                s.source_path.display()
            ));
            report.duplicates_skipped += 1;
            pb.inc(1);
            continue;
        }
        window.push(s);
        pb.inc(1);
        if window.len() >= crate::embed_pool::CROSS_SESSION_BATCH {
            flush_batch(client, &pool, &embedder, &mut window, &mut report, &pb).await;
        }
    }
    if !window.is_empty() {
        flush_batch(client, &pool, &embedder, &mut window, &mut report, &pb).await;
    }
    pb.finish_with_message("done");
    Ok(report)
}

/// Legacy (per-session) bulk index — retained so callers that hold a plain
/// `&Embedder` still compile. Internally delegates to `index_session` one at
/// a time.
async fn bulk_index_legacy(
    client: &Qdrant,
    embedder: &Embedder,
    sessions: &[Session],
) -> Result<BulkIndexReport> {
    use indicatif::{ProgressBar, ProgressStyle};
    let pb = ProgressBar::new(sessions.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{wide_bar} {pos}/{len} ({eta}) {msg}")
            .unwrap()
            .progress_chars("=> "),
    );
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut report = BulkIndexReport {
        indexed: 0,
        duplicates_skipped: 0,
        errors: 0,
    };
    for s in sessions {
        let label = s
            .project_name
            .clone()
            .unwrap_or_else(|| s.session_id.clone());
        pb.set_message(label);
        if !seen_ids.insert(s.session_id.clone()) {
            pb.println(format!(
                "  ⊘ duplicate sessionId={} ({}) — kept first occurrence",
                &s.session_id[..8.min(s.session_id.len())],
                s.source_path.display()
            ));
            report.duplicates_skipped += 1;
            pb.inc(1);
            continue;
        }
        match index_session(client, embedder, s).await {
            Ok(()) => report.indexed += 1,
            Err(e) => {
                pb.println(format!("  ⚠ {}: {:#}", s.session_id, e));
                report.errors += 1;
            }
        }
        pb.inc(1);
    }
    pb.finish_with_message("done");
    Ok(report)
}

/// Flush one batch — collect 5 extracts per session, embed them in ONE pool
/// call, then per-session pack + upsert with multivector.
async fn flush_batch(
    client: &Qdrant,
    pool: &crate::embed_pool::EmbedPool,
    embedder: &Embedder,
    window: &mut Vec<&Session>,
    report: &mut BulkIndexReport,
    pb: &indicatif::ProgressBar,
) {
    let local: Vec<&Session> = std::mem::take(window);
    if local.is_empty() {
        return;
    }
    let mut all_texts: Vec<String> = Vec::with_capacity(local.len() * 5);
    let mut extracts_per_session: Vec<[(String, String); 5]> = Vec::with_capacity(local.len());
    for s in &local {
        let ex = session_extracts(s);
        for (_, t) in &ex {
            all_texts.push(t.clone());
        }
        extracts_per_session.push(ex);
    }
    let vectors = match pool.embed_batch(all_texts).await {
        Ok(v) => v,
        Err(e) => {
            pb.println(format!("  ⚠ batch embed: {:#}", e));
            // Conservative: charge every session in the window as an error so
            // the report adds up.
            report.errors = report.errors.saturating_add(local.len());
            return;
        }
    };
    for (i, s) in local.iter().enumerate() {
        let extracts = &extracts_per_session[i];
        let base = i * 5;
        let named_dense: Vec<(String, Vec<f32>)> = extracts
            .iter()
            .enumerate()
            .map(|(j, (k, _))| (k.clone(), vectors[base + j].clone()))
            .collect();
        let content_text = extracts
            .iter()
            .find(|(k, _)| k == "content")
            .map(|(_, t)| t.clone())
            .unwrap_or_default();
        let chunk_vectors = match crate::embed_late::embed_token_level(embedder, &content_text) {
            Ok(v) => v,
            Err(e) => {
                pb.println(format!("  ⚠ {}: late embed: {:#}", s.session_id, e));
                report.errors += 1;
                continue;
            }
        };
        let point = build_point_v3_with_multivec(s, named_dense, chunk_vectors);
        match client
            .upsert_points(
                UpsertPointsBuilder::new(crate::schema::COLLECTION_V3, vec![point]).wait(true),
            )
            .await
        {
            Ok(_) => report.indexed += 1,
            Err(e) => {
                pb.println(format!("  ⚠ {}: {:#}", s.session_id, e));
                report.errors += 1;
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchHit {
    pub score: f32,
    pub session_id: String,
    pub project_name: String,
    pub ai_title: String,
    pub start_iso: String,
    /// Per-vector contribution scores (only populated by lens_search).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub vector_scores: std::collections::HashMap<String, f32>,
}

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
    /// KB-01 — late-interaction MaxSim weight. Defaults to 0.0 (off) so
    /// existing callers that don't set it keep the dense-only behavior.
    #[serde(default = "default_zero_weight")]
    pub content_late: f32,
}

fn default_weight() -> f32 {
    1.0
}

fn default_zero_weight() -> f32 {
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
        }
    }
}

pub async fn search_content(
    client: &Qdrant,
    embedder: &Embedder,
    query: &str,
    limit: u64,
) -> Result<Vec<SearchHit>> {
    let weights = LensWeights {
        content: 1.0,
        tool: 0.0,
        path: 0.0,
        error: 0.0,
        code: 0.0,
        content_late: 0.0,
    };
    lens_search(client, embedder, query, &weights, limit, 50).await
}

/// **Lens slider** (Plan §3 T3.1) — **P2 wiring**: this function is now a
/// thin shim over `crate::lens::lens_search_v2`, which executes a single
/// server-side FormulaQuery (KA-01) with per-prefetch recency decay +
/// has_errors boost. Public signature stays stable for backward compat;
/// `per_vector_limit` is preserved but no longer used directly (the new
/// path manages its own PREFETCH_LIMIT internally).
///
/// Returns the top `limit` sessions; per-vector contributions remain in
/// `SearchHit.vector_scores` so the inspector keeps rendering correctly.
pub async fn lens_search(
    client: &Qdrant,
    embedder: &Embedder,
    query: &str,
    weights: &LensWeights,
    limit: u64,
    _per_vector_limit: u64,
) -> Result<Vec<SearchHit>> {
    // Convert the legacy LensWeights → lens::LensWeights (the legacy struct
    // has no `diversity`/`fusion` knobs; defaults are Formula + no MMR which
    // matches the previous behavior in terms of result rank stability).
    let lens_weights: crate::lens::LensWeights = weights.clone().into();
    // Empty query / all-zero-weights — legacy code returned `Ok(empty)`. The
    // new API returns Err; map back to empty for backward compat.
    let res = match crate::lens::lens_search_v2(client, embedder, query, &lens_weights, limit)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            let msg = format!("{e}");
            if msg == "no active lens" || msg == "empty query" {
                return Ok(Vec::new());
            }
            return Err(e);
        }
    };

    Ok(res
        .into_iter()
        .map(crate::lens::lens_result_to_searchhit)
        .collect())
}

/// **Mix & Match** (Plan §3 T3.2) — Discovery API.
///
/// Each positive session pairs with one negative session (or a synthetic
/// anti-context). Discovery picks vectors closer to the positive(s) and
/// farther from the negative(s) in one query.
pub async fn mix_match(
    client: &Qdrant,
    positive_session_ids: &[String],
    negative_session_ids: &[String],
    limit: u64,
) -> Result<Vec<SearchHit>> {
    if positive_session_ids.is_empty() && negative_session_ids.is_empty() {
        anyhow::bail!("mix_match needs at least one positive or negative session id");
    }
    let to_pid = |s: &String| PointId {
        point_id_options: Some(PointIdOptions::Uuid(point_id(s))),
    };

    // Build context pairs. If counts differ, the longer side is paired with the
    // first element of the shorter side as a stand-in.
    let pos: Vec<PointId> = positive_session_ids.iter().map(to_pid).collect();
    let neg: Vec<PointId> = negative_session_ids.iter().map(to_pid).collect();
    let len = pos.len().max(neg.len()).max(1);
    let mut pairs: Vec<ContextInputPair> = Vec::with_capacity(len);
    for i in 0..len {
        let positive = pos.get(i).cloned().unwrap_or_else(|| pos[0].clone());
        let negative = neg
            .get(i)
            .cloned()
            .or_else(|| neg.first().cloned())
            .unwrap_or_else(|| positive.clone());
        pairs.push(ContextInputPair {
            positive: Some(VectorInput::from(positive)),
            negative: Some(VectorInput::from(negative)),
        });
    }

    let context = ContextInputBuilder::default().pairs(pairs).build();
    // Qdrant 1.18 requires a target; use the first positive (or first negative
    // as a fallback) so context discovery has something to anchor on.
    let target_pid = pos
        .first()
        .cloned()
        .or_else(|| neg.first().cloned())
        .context("mix_match needs at least one session id")?;
    let discover_input = qdrant_client::qdrant::DiscoverInput {
        target: Some(VectorInput::from(target_pid)),
        context: Some(context),
    };

    let resp = client
        .query(
            QueryPointsBuilder::new(crate::schema::COLLECTION_V3)
                .query(discover_input)
                .using("content".to_string())
                .limit(limit)
                .with_payload(true)
                .params(crate::schema::search_params_with_quantization()),
        )
        .await?;
    Ok(resp
        .result
        .into_iter()
        .map(|p| SearchHit {
            score: p.score,
            session_id: payload_str(&p.payload, "session_id").unwrap_or_default(),
            project_name: payload_str(&p.payload, "project_name").unwrap_or_default(),
            ai_title: payload_str(&p.payload, "ai_title").unwrap_or_default(),
            start_iso: payload_str(&p.payload, "start_iso").unwrap_or_default(),
            vector_scores: HashMap::new(),
        })
        .collect())
}

/// Topology MST node + edge for frontend graph rendering.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TopoNode {
    pub session_id: String,
    pub project_name: String,
    pub ai_title: String,
    pub start_iso: String,
    pub user_turns: i64,
    pub tool_count: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TopoEdge {
    pub a: String,
    pub b: String,
    pub distance: f32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectInsight {
    pub project_name: String,
    pub session_count: usize,
    /// Auto-generated one-line label, e.g. "code + shell · Edit×42 Bash×18".
    pub label: String,
    /// Theme keyword chosen from tool mix.
    pub theme: String,
    /// Top tools by usage in this project's sessions.
    pub top_tools: Vec<(String, usize)>,
    /// Top 3 directories touched (file_path stems).
    pub top_paths: Vec<String>,
    /// Number of sessions in this project that produced an error.
    pub had_errors: usize,
    /// Number of MST edges that connect this project to a *different* project.
    pub bridges_out: usize,
    /// Other project names this one is bridged to.
    pub bridge_partners: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GapInsight {
    /// "isolated" | "near_miss"
    pub kind: String,
    pub project_a: String,
    pub project_b: Option<String>,
    /// 0..1 (cosine similarity)
    pub similarity: f32,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Topology {
    pub nodes: Vec<TopoNode>,
    pub edges: Vec<TopoEdge>,
    pub project_insights: Vec<ProjectInsight>,
    pub gap_insights: Vec<GapInsight>,
}

/// **Topology view** (Plan §3 T3.3) — Distance Matrix → MST.
///
/// `sample` = how many sessions to consider; `nearest_per_point` = how many
/// nearest neighbors per point to fetch for the pairwise matrix.
///
/// `projects_root` — optional path. When provided, also performs a fresh
/// `scan_dir` so the response carries `project_insights` (auto-labels) and
/// `gap_insights` (isolated clusters + near-miss bridges). Pass `None` to
/// skip the extra scan when the caller doesn't need them.
pub async fn topology(
    client: &Qdrant,
    sample: u32,
    nearest_per_point: u32,
    projects_root: Option<std::path::PathBuf>,
) -> Result<Topology> {
    // 1. Get pairwise distances from Qdrant's distance matrix endpoint.
    //
    // SPEC NOTE (P3, KC-01b): SearchMatrixPointsBuilder does not currently
    // expose a `.params(SearchParams)` setter in qdrant-client 1.18 (the
    // pairs endpoint runs server-side with its own internal config). The
    // rescore + oversampling knobs only apply to query-by-vector calls.
    let resp = client
        .search_matrix_pairs(
            SearchMatrixPointsBuilder::new(crate::schema::COLLECTION_V3)
                .using("content".to_string())
                .sample(sample as u64)
                .limit(nearest_per_point as u64),
        )
        .await?;

    // 2. Collect unique node ids, fetch their payloads in one batch.
    use std::collections::BTreeSet;
    let mut id_set: BTreeSet<String> = BTreeSet::new();
    let empty = qdrant_client::qdrant::SearchMatrixPairs { pairs: Vec::new() };
    let matrix = resp.result.as_ref().unwrap_or(&empty);
    for pair in &matrix.pairs {
        if let Some(a) = pair.a.as_ref().and_then(point_id_string) {
            id_set.insert(a);
        }
        if let Some(b) = pair.b.as_ref().and_then(point_id_string) {
            id_set.insert(b);
        }
    }
    let point_ids: Vec<PointId> = id_set
        .iter()
        .map(|u| PointId {
            point_id_options: Some(PointIdOptions::Uuid(u.clone())),
        })
        .collect();
    let detail = if point_ids.is_empty() {
        Vec::new()
    } else {
        // KG-03 dual-read — v3 first, v2 fallback so topology still has labels
        // for any point that hasn't been migrated yet.
        crate::crud::dual_get_points(client, point_ids).await?
    };

    // node_by_pid: Qdrant point uuid → TopoNode (we still use point uuid as the
    // graph node id internally because that's what `search_matrix_pairs`
    // returns). We *also* build `pid_to_session` so the final edges can speak
    // the same identifier domain the frontend uses for positioning (which is
    // `TopoNode.session_id`, the payload field).
    let mut node_by_pid: HashMap<String, TopoNode> = HashMap::new();
    let mut pid_to_session: HashMap<String, String> = HashMap::new();
    for p in detail {
        let pid = match p.id.as_ref().and_then(point_id_string) {
            Some(s) => s,
            None => continue,
        };
        let sid = payload_str(&p.payload, "session_id").unwrap_or_default();
        pid_to_session.insert(pid.clone(), sid.clone());
        node_by_pid.insert(
            pid,
            TopoNode {
                session_id: sid,
                project_name: payload_str(&p.payload, "project_name").unwrap_or_default(),
                ai_title: payload_str(&p.payload, "ai_title").unwrap_or_default(),
                start_iso: payload_str(&p.payload, "start_iso").unwrap_or_default(),
                user_turns: payload_i64(&p.payload, "user_turns").unwrap_or(0),
                tool_count: payload_i64(&p.payload, "tool_count").unwrap_or(0),
            },
        );
    }

    // 3. Build an undirected weighted graph and compute MST.
    use petgraph::graph::UnGraph;
    use petgraph::algo::min_spanning_tree;
    use petgraph::data::FromElements;
    let mut g: UnGraph<String, f32> = UnGraph::new_undirected();
    let mut idx: HashMap<String, _> = HashMap::new();
    for id in &id_set {
        idx.insert(id.clone(), g.add_node(id.clone()));
    }
    // B1: Qdrant returns `pair.score` as **similarity** (cosine, higher = closer).
    // MST is a *minimum* spanning tree — it picks the lowest-weight edges. To
    // get the "most cohesive backbone" we need to feed it **distance**
    // (low = close), i.e. `1 - similarity`.
    let matrix2 = resp.result.as_ref().unwrap_or(&empty);
    for pair in &matrix2.pairs {
        let a = pair.a.as_ref().and_then(point_id_string);
        let b = pair.b.as_ref().and_then(point_id_string);
        if let (Some(a), Some(b)) = (a, b) {
            if let (Some(&na), Some(&nb)) = (idx.get(&a), idx.get(&b)) {
                let distance = (1.0 - pair.score).max(0.0);
                g.add_edge(na, nb, distance);
            }
        }
    }
    let mst = UnGraph::<String, f32>::from_elements(min_spanning_tree(&g));

    // Translate MST endpoints from Qdrant point uuids → session_ids so the
    // frontend can render edges against the same id namespace as `nodes[].session_id`.
    let mut edges = Vec::new();
    for e in mst.edge_indices() {
        let (na, nb) = mst.edge_endpoints(e).unwrap();
        let w = mst[e];
        let a_pid = &mst[na];
        let b_pid = &mst[nb];
        let (Some(a_sid), Some(b_sid)) =
            (pid_to_session.get(a_pid), pid_to_session.get(b_pid))
        else {
            continue;
        };
        edges.push(TopoEdge {
            a: a_sid.clone(),
            b: b_sid.clone(),
            distance: w,
        });
    }

    let mut nodes: Vec<TopoNode> = id_set
        .iter()
        .filter_map(|id| node_by_pid.remove(id))
        .collect();
    nodes.sort_by(|a, b| a.start_iso.cmp(&b.start_iso));

    // ---- Insights (A: auto-labels, C: gap analysis) ---------------------
    // Bridge counting requires per-node project lookups — build once.
    let mut node_project: HashMap<String, String> = HashMap::new();
    for n in &nodes {
        node_project.insert(n.session_id.clone(), n.project_name.clone());
    }

    let (project_insights, gap_insights) = if let Some(root) = projects_root {
        // P5 KG-01 — memoize the heavy compute on (root, max_mtime). Cache
        // misses fall through to the full scan+compute below; hits return the
        // cached `Arc<CachedInsights>` so we just clone the inner Vecs out.
        let max_mt = crate::insights_cache::InsightsCache::fingerprint(&root);
        let root_for_compute = root.clone();
        let nodes_ref = nodes.clone();
        let edges_ref = edges.clone();
        let matrix_pairs: Vec<qdrant_client::qdrant::SearchMatrixPair> =
            matrix2.pairs.clone();
        let node_project_ref = node_project.clone();
        let pid_to_session_ref = pid_to_session.clone();
        match INSIGHTS_CACHE.get_or_compute(root, max_mt, move || {
            let sessions = crate::parser::scan_dir(&root_for_compute)?;
            let (pi, gi) = compute_insights(
                &sessions,
                &nodes_ref,
                &edges_ref,
                &matrix_pairs,
                &node_project_ref,
                &pid_to_session_ref,
            );
            Ok(crate::insights_cache::CachedInsights {
                project_insights: pi,
                gap_insights: gi,
            })
        }) {
            Ok(cached) => (
                cached.project_insights.clone(),
                cached.gap_insights.clone(),
            ),
            Err(_) => (Vec::new(), Vec::new()),
        }
    } else {
        (Vec::new(), Vec::new())
    };

    Ok(Topology {
        nodes,
        edges,
        project_insights,
        gap_insights,
    })
}

// ----------------------------------------------------------------------------
// Cluster auto-labels (A) + gap analysis (C)
// ----------------------------------------------------------------------------

fn compute_insights(
    sessions: &[crate::parser::Session],
    nodes: &[TopoNode],
    mst_edges: &[TopoEdge],
    matrix_pairs: &[qdrant_client::qdrant::SearchMatrixPair],
    _node_project: &HashMap<String, String>,
    pid_to_session: &HashMap<String, String>,
) -> (Vec<ProjectInsight>, Vec<GapInsight>) {
    use std::collections::BTreeMap;

    let active_projects: HashSet<String> = nodes.iter().map(|n| n.project_name.clone()).collect();

    // Aggregate per-project from raw parsed sessions: tools + paths + error count.
    #[derive(Default)]
    struct Agg {
        session_count: usize,
        had_errors: usize,
        tool_freq: HashMap<String, usize>,
        path_freq: HashMap<String, usize>,
    }
    let mut agg: HashMap<String, Agg> = HashMap::new();
    for s in sessions {
        let project = s.project_name.clone().unwrap_or_default();
        if project.is_empty() || !active_projects.contains(&project) {
            continue;
        }
        let entry = agg.entry(project).or_default();
        entry.session_count += 1;
        let mut had_err = false;
        for turn in &s.turns {
            for tc in &turn.tool_calls {
                *entry.tool_freq.entry(tc.name.clone()).or_insert(0) += 1;
                // file_path top-2 dirs
                if let Some(p) = tc.input.get("file_path").and_then(|v| v.as_str()) {
                    if let Some(dir) = std::path::Path::new(p)
                        .parent()
                        .and_then(|d| d.to_str())
                    {
                        // Bucket by 2 levels deep to avoid every unique full path.
                        let bucket = bucket_path(dir);
                        *entry.path_freq.entry(bucket).or_insert(0) += 1;
                    }
                }
            }
            if turn.tool_results.iter().any(|r| r.is_error) {
                had_err = true;
            }
        }
        if had_err {
            entry.had_errors += 1;
        }
    }

    // Per-project MST bridges (cross-project edges in the MST tree).
    let mut bridges: HashMap<String, BTreeMap<String, usize>> = HashMap::new();
    let session_project: HashMap<String, String> = nodes
        .iter()
        .map(|n| (n.session_id.clone(), n.project_name.clone()))
        .collect();
    for e in mst_edges {
        let pa = session_project.get(&e.a).cloned().unwrap_or_default();
        let pb = session_project.get(&e.b).cloned().unwrap_or_default();
        if pa.is_empty() || pb.is_empty() || pa == pb {
            continue;
        }
        bridges
            .entry(pa.clone())
            .or_default()
            .entry(pb.clone())
            .and_modify(|n| *n += 1)
            .or_insert(1);
        bridges
            .entry(pb)
            .or_default()
            .entry(pa)
            .and_modify(|n| *n += 1)
            .or_insert(1);
    }

    // Build project insights, sorted by session_count desc.
    let mut project_insights: Vec<ProjectInsight> = agg
        .into_iter()
        .map(|(project_name, a)| {
            let mut tools: Vec<(String, usize)> = a.tool_freq.into_iter().collect();
            tools.sort_by(|x, y| y.1.cmp(&x.1));
            tools.truncate(4);

            let mut paths: Vec<(String, usize)> = a.path_freq.into_iter().collect();
            paths.sort_by(|x, y| y.1.cmp(&x.1));
            let top_paths: Vec<String> = paths.into_iter().take(3).map(|(p, _)| p).collect();

            let theme = theme_from_tools(&tools).to_string();
            let tools_breakdown = tools
                .iter()
                .take(3)
                .map(|(n, c)| format!("{n}×{c}"))
                .collect::<Vec<_>>()
                .join(" ");
            let label = if tools_breakdown.is_empty() {
                theme.clone()
            } else {
                format!("{theme} · {tools_breakdown}")
            };

            let project_bridges = bridges.get(&project_name).cloned().unwrap_or_default();
            let bridges_out = project_bridges.values().sum();
            let bridge_partners = project_bridges.keys().cloned().collect();

            ProjectInsight {
                project_name,
                session_count: a.session_count,
                label,
                theme,
                top_tools: tools,
                top_paths,
                had_errors: a.had_errors,
                bridges_out,
                bridge_partners,
            }
        })
        .collect();
    project_insights.sort_by(|a, b| b.session_count.cmp(&a.session_count));

    // ---- Gaps -----------------------------------------------------------
    // Aggregate similarity across cross-project matrix pairs (not just MST).
    // We use these to find near-miss connections — pairs that ARE close
    // semantically but didn't make it into the MST (a tree only keeps
    // n-1 edges so many close cross-project pairs get pruned).
    let mut cross_pair_scores: HashMap<(String, String), Vec<f32>> = HashMap::new();
    for pair in matrix_pairs {
        let Some(a) = pair.a.as_ref().and_then(point_id_string) else { continue };
        let Some(b) = pair.b.as_ref().and_then(point_id_string) else { continue };
        let Some(sa) = pid_to_session.get(&a) else { continue };
        let Some(sb) = pid_to_session.get(&b) else { continue };
        let pa = session_project.get(sa).cloned().unwrap_or_default();
        let pb = session_project.get(sb).cloned().unwrap_or_default();
        if pa.is_empty() || pb.is_empty() || pa == pb {
            continue;
        }
        let key = if pa < pb { (pa, pb) } else { (pb, pa) };
        cross_pair_scores.entry(key).or_default().push(pair.score);
    }

    let mut mst_project_pairs: HashSet<(String, String)> = HashSet::new();
    for e in mst_edges {
        let pa = session_project.get(&e.a).cloned().unwrap_or_default();
        let pb = session_project.get(&e.b).cloned().unwrap_or_default();
        if pa.is_empty() || pb.is_empty() || pa == pb {
            continue;
        }
        let key = if pa < pb { (pa, pb) } else { (pb, pa) };
        mst_project_pairs.insert(key);
    }

    let mut gap_insights: Vec<GapInsight> = Vec::new();

    // Isolated clusters — projects with multiple sessions but zero MST bridges.
    for p in &project_insights {
        if p.session_count >= 2 && p.bridges_out == 0 {
            gap_insights.push(GapInsight {
                kind: "isolated".to_string(),
                project_a: p.project_name.clone(),
                project_b: None,
                similarity: 0.0,
                message: format!(
                    "‘{}’ ({} sessions) sits alone — its sessions don't share enough vocabulary with anything else for a bridge to form.",
                    p.project_name, p.session_count
                ),
            });
        }
    }

    // Near-miss bridges — high cross-project avg similarity but no MST edge.
    let mut near_miss_pairs: Vec<((String, String), f32)> = Vec::new();
    for (pair, scores) in cross_pair_scores {
        if mst_project_pairs.contains(&pair) {
            continue;
        }
        if scores.is_empty() {
            continue;
        }
        let avg = scores.iter().sum::<f32>() / scores.len() as f32;
        if avg >= 0.50 {
            near_miss_pairs.push((pair, avg));
        }
    }
    near_miss_pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for ((pa, pb), sim) in near_miss_pairs.into_iter().take(5) {
        gap_insights.push(GapInsight {
            kind: "near_miss".to_string(),
            project_a: pa.clone(),
            project_b: Some(pb.clone()),
            similarity: sim,
            message: format!(
                "‘{}’ and ‘{}’ have semantically similar sessions (avg sim {:.2}) but no bridge in the MST — a potential unmade connection.",
                pa, pb, sim
            ),
        });
    }

    (project_insights, gap_insights)
}

fn bucket_path(dir: &str) -> String {
    // Keep first 3 path segments to avoid every unique deep file.
    let parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 {
        dir.to_string()
    } else {
        format!("/{}/{}/{}/…", parts[0], parts[1], parts[2])
    }
}

fn theme_from_tools(top_tools: &[(String, usize)]) -> &'static str {
    let names: HashSet<&str> = top_tools.iter().map(|(n, _)| n.as_str()).collect();
    if names.contains("Bash") && (names.contains("Edit") || names.contains("MultiEdit")) {
        "code + shell"
    } else if names.contains("WebFetch") || names.contains("WebSearch") {
        "research"
    } else if names.contains("Task") || names.contains("Agent") {
        "agent orchestration"
    } else if names.contains("Edit") || names.contains("MultiEdit") || names.contains("Write") {
        "editing"
    } else if names.contains("Bash") {
        "shell work"
    } else if names.contains("Read") || names.contains("Grep") || names.contains("Glob") {
        "exploration"
    } else {
        "general"
    }
}

/// **Proactive recall** (Plan §3 T3.6) — find past sessions that solved an
/// error matching the input signature. Embeds `error_text` and searches the
/// dedicated `error` named vector; only sessions flagged `has_errors=true`
/// are kept.
pub async fn recall(
    client: &Qdrant,
    embedder: &Embedder,
    error_text: &str,
    limit: u64,
) -> Result<Vec<SearchHit>> {
    let vecs = embedder.embed(vec![error_text.to_string()])?;
    let qvec = vecs.into_iter().next().context("no embedding for query")?;
    let q: Query = qvec.into();
    let filter = Filter {
        must: vec![qdrant_client::qdrant::Condition::matches(
            "has_errors",
            true,
        )],
        ..Default::default()
    };
    // KB-04 (ACORN): every filtered query takes the ACORN path —
    // hnsw_ef=128 + exact=false on top of the v3 quantization knobs. The
    // `is_tenant=true` payload index on project_name (P3, KC-03) is the
    // structural half; this is the per-query tuning.
    let res = client
        .query(
            QueryPointsBuilder::new(crate::schema::COLLECTION_V3)
                .query(q)
                .using("error")
                .limit(limit)
                .filter(filter)
                .with_payload(true)
                .params(crate::retrieval::search_params_filtered_acorn(Some(128))),
        )
        .await?;
    Ok(res
        .result
        .into_iter()
        .map(|p| SearchHit {
            score: p.score,
            session_id: payload_str(&p.payload, "session_id").unwrap_or_default(),
            project_name: payload_str(&p.payload, "project_name").unwrap_or_default(),
            ai_title: payload_str(&p.payload, "ai_title").unwrap_or_default(),
            start_iso: payload_str(&p.payload, "start_iso").unwrap_or_default(),
            vector_scores: HashMap::new(),
        })
        .collect())
}

// ----------------------------------------------------------------------------
// Predictive next-action — "What would past-you do next?"
// ----------------------------------------------------------------------------

/// One predicted next-step the user might take, derived from how past sessions
/// proceeded *from a similar conversational position*.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PredictedAction {
    pub rank: usize,
    pub tool_name: String,
    /// Short human-readable summary of the input (e.g., "cargo build" or
    /// "src/lib.rs").
    pub example_input_summary: String,
    /// The full input JSON from the source session, so the UI can show the
    /// concrete command/file/etc.
    pub example_input_raw: serde_json::Value,
    /// Aggregate evidence: fraction of (neighbor_session × next-N-turn) slots
    /// in which this tool appeared.
    pub frequency: f32,
    /// Mean cosine similarity of the neighbor sessions that contributed.
    pub confidence: f32,
    /// Source attribution — which past session this example came from.
    pub from_session_id: String,
    pub from_session_project: String,
    pub from_turn_uuid: String,
    pub from_turn_index: usize,
    pub from_turn_text_preview: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PredictionContext {
    pub source_session_id: String,
    pub source_last_turn_preview: String,
    pub neighbors_searched: usize,
    pub neighbors_used: usize,
    pub predictions: Vec<PredictedAction>,
}

/// **Predictive next-action** — Memex's recommendation answer to
/// *"what would past-you have done next at this point?"*
///
/// Algorithm (intentionally LLM-free):
/// 1. Re-parse the active session from `source_path` (payload lookup).
/// 2. Embed the concatenated text of its last `last_n_turns` turns.
/// 3. Query the `content` named vector for `neighbors` similar past sessions.
/// 4. For each neighbor, find the *pivot turn* — the one whose text shares the
///    most distinctive vocabulary with the active session's most recent turn.
///    This anchors "where we are" in that neighbor's timeline.
/// 5. Walk forward `horizon` turns from the pivot and collect every tool call.
/// 6. Aggregate by tool name × neighbor-similarity; rank by `frequency × conf`.
///
/// The output is a small ranked list — each entry carries a concrete example
/// (so the UI can render a Bash command or file path), a source attribution,
/// and the exact turn index so the user can jump to that moment in Replay.
pub async fn predict_next_actions(
    client: &Qdrant,
    embedder: &Embedder,
    session_id: &str,
    last_n_turns: usize,
    horizon: usize,
    neighbors: u64,
) -> Result<PredictionContext> {
    use std::collections::HashSet;
    use std::path::Path as StdPath;

    // ---- 1. Active session ---------------------------------------------
    let payload = get_session_payload(client, session_id)
        .await?
        .context("active session not in index — re-index first")?;
    let source_path = payload
        .get("source_path")
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            qdrant_client::qdrant::value::Kind::StringValue(s) => Some(s.clone()),
            _ => None,
        })
        .context("active session payload is missing source_path")?;
    let validated = crate::sec::validate_session_path(StdPath::new(&source_path))?;
    let active = crate::parser::parse_session(&validated)?;
    if active.turns.is_empty() {
        return Ok(PredictionContext {
            source_session_id: session_id.to_string(),
            source_last_turn_preview: String::new(),
            neighbors_searched: 0,
            neighbors_used: 0,
            predictions: Vec::new(),
        });
    }

    // Concatenate the last N turns' text for the query embedding.
    let active_tail: Vec<&crate::parser::Turn> =
        active.turns.iter().rev().take(last_n_turns).collect();
    let recent_text: String = active_tail
        .iter()
        .rev()
        .map(|t| t.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let anchor_text = active
        .turns
        .last()
        .map(|t| t.text.clone())
        .unwrap_or_default();
    let source_last_turn_preview: String = anchor_text.chars().take(220).collect();

    // ---- 2. Embed + nearest content-vector neighbors -------------------
    let vecs = embedder.embed(vec![if recent_text.trim().is_empty() {
        anchor_text.clone()
    } else {
        recent_text
    }])?;
    let qvec = vecs.into_iter().next().context("no embedding for query")?;
    let q: Query = qvec.into();
    let res = client
        .query(
            QueryPointsBuilder::new(crate::schema::COLLECTION_V3)
                .query(q)
                .using("content")
                .limit(neighbors + 1) // +1 because we filter out the active session itself
                .with_payload(true)
                .params(crate::schema::search_params_with_quantization()),
        )
        .await?;

    let mut neighbor_meta: Vec<(String, f32, String, String)> = Vec::new();
    for p in res.result {
        let Some(sid) = payload_str(&p.payload, "session_id") else { continue };
        if sid == session_id {
            continue;
        }
        let Some(src) = payload_str(&p.payload, "source_path") else { continue };
        let project = payload_str(&p.payload, "project_name").unwrap_or_default();
        neighbor_meta.push((sid, p.score, src, project));
        if neighbor_meta.len() as u64 >= neighbors {
            break;
        }
    }
    let neighbors_searched = neighbor_meta.len();

    // ---- 3. Per-neighbor pivot detection + horizon walk ---------------
    use std::collections::HashMap;
    let mut by_tool: HashMap<String, Vec<PredictedAction>> = HashMap::new();
    let mut neighbors_used = 0usize;

    // Pre-compute the anchor's distinctive words once.
    let anchor_words: HashSet<String> = anchor_text
        .split_whitespace()
        .filter(|w| w.len() > 4)
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
        .filter(|w| w.len() > 4)
        .collect();

    for (nb_sid, sim_score, source, nb_project) in &neighbor_meta {
        // SAFETY: neighbor source_path came from Qdrant payload — validate
        // it lives inside an allowed sandbox root before parsing the JSONL.
        let Ok(validated) = crate::sec::validate_session_path(StdPath::new(source)) else { continue };
        // P5 KG-02 — LRU memo keyed by (path, mtime). Cache hits avoid the
        // re-parse on repeat predict calls touching the same neighbours.
        let mtime = std::fs::metadata(&validated)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        let nb_arc = match PREDICT_PARSE_CACHE.get_or_parse(validated.clone(), mtime, |p| {
            crate::parser::parse_session(p)
        }) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let nb = &*nb_arc;
        if nb.turns.is_empty() {
            continue;
        }
        let pivot = find_pivot_turn(&nb.turns, &anchor_words);
        neighbors_used += 1;

        // Walk forward `horizon` turns from pivot+1.
        for offset in 1..=horizon {
            let idx = pivot + offset;
            if idx >= nb.turns.len() {
                break;
            }
            let turn = &nb.turns[idx];
            for tc in &turn.tool_calls {
                let action = PredictedAction {
                    rank: 0,
                    tool_name: tc.name.clone(),
                    example_input_summary: summarize_tool_input(&tc.name, &tc.input),
                    example_input_raw: tc.input.clone(),
                    frequency: 0.0,
                    confidence: *sim_score,
                    from_session_id: nb_sid.clone(),
                    from_session_project: nb_project.clone(),
                    from_turn_uuid: turn.uuid.clone(),
                    from_turn_index: idx,
                    from_turn_text_preview: turn.text.chars().take(180).collect(),
                };
                by_tool.entry(tc.name.clone()).or_default().push(action);
            }
        }
    }

    // ---- 4. Aggregate + rank ------------------------------------------
    let total_actions: usize = by_tool.values().map(|v| v.len()).sum();
    let mut predictions: Vec<PredictedAction> = Vec::new();
    for (_, mut actions) in by_tool {
        if actions.is_empty() {
            continue;
        }
        let freq = actions.len() as f32 / total_actions.max(1) as f32;
        let avg_conf: f32 =
            actions.iter().map(|a| a.confidence).sum::<f32>() / actions.len() as f32;
        // Representative example: pick the action whose neighbor was the most
        // similar to the active session.
        actions.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut best = actions.into_iter().next().unwrap();
        best.frequency = freq;
        best.confidence = avg_conf;
        predictions.push(best);
    }
    predictions.sort_by(|a, b| {
        let sa = a.frequency * 0.55 + a.confidence * 0.45;
        let sb = b.frequency * 0.55 + b.confidence * 0.45;
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    predictions.truncate(6);
    for (i, p) in predictions.iter_mut().enumerate() {
        p.rank = i + 1;
    }

    Ok(PredictionContext {
        source_session_id: session_id.to_string(),
        source_last_turn_preview,
        neighbors_searched,
        neighbors_used,
        predictions,
    })
}

/// Lexical pivot finder — picks the turn in `turns` with the most word
/// overlap with the anchor. Falls back to "two-thirds in" if no good signal
/// (assuming the user is mid-flow, the resolution would be later).
fn find_pivot_turn(turns: &[crate::parser::Turn], anchor_words: &std::collections::HashSet<String>) -> usize {
    let fallback = (turns.len() * 2 / 3).min(turns.len().saturating_sub(1));
    if anchor_words.is_empty() {
        return fallback;
    }
    let mut best_overlap = 0usize;
    let mut best_idx = fallback;
    for (i, t) in turns.iter().enumerate() {
        let count = t
            .text
            .split_whitespace()
            .filter(|w| w.len() > 4)
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .filter(|w| anchor_words.contains(w))
            .count();
        if count > best_overlap {
            best_overlap = count;
            best_idx = i;
        }
    }
    if best_overlap < 2 {
        fallback
    } else {
        best_idx
    }
}

fn summarize_tool_input(tool_name: &str, input: &serde_json::Value) -> String {
    let s = match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Edit" | "MultiEdit" | "Write" | "Read" | "NotebookEdit" => input
            .get("file_path")
            .or_else(|| input.get("notebook_path"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "WebFetch" | "WebSearch" => input
            .get("url")
            .or_else(|| input.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Task" | "Agent" => input
            .get("description")
            .or_else(|| input.get("subagent_type"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        "Grep" | "Glob" => input
            .get("pattern")
            .or_else(|| input.get("query"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        _ => serde_json::to_string(input).unwrap_or_default(),
    };
    s.chars().take(160).collect()
}

/// Fetch a single session's payload (for the inspector pane in the UI).
///
/// KG-03 dual-read: v3 first, fall back to v2 — so points that haven't been
/// migrated yet still resolve.
pub async fn get_session_payload(
    client: &Qdrant,
    session_id: &str,
) -> Result<Option<HashMap<String, qdrant_client::qdrant::Value>>> {
    crate::crud::dual_get_session_payload(client, session_id).await
}

fn payload_i64(
    p: &HashMap<String, qdrant_client::qdrant::Value>,
    key: &str,
) -> Option<i64> {
    p.get(key).and_then(|v| v.kind.as_ref()).and_then(|k| match k {
        qdrant_client::qdrant::value::Kind::IntegerValue(i) => Some(*i),
        _ => None,
    })
}

fn point_id_string(p: &PointId) -> Option<String> {
    match p.point_id_options.as_ref()? {
        PointIdOptions::Uuid(u) => Some(u.clone()),
        PointIdOptions::Num(n) => Some(n.to_string()),
    }
}


pub(crate) fn payload_str(
    p: &HashMap<String, qdrant_client::qdrant::Value>,
    key: &str,
) -> Option<String> {
    p.get(key)
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            qdrant_client::qdrant::value::Kind::StringValue(s) => Some(s.clone()),
            _ => None,
        })
}

/// Snapshot export — calls Qdrant's HTTP snapshot endpoint and copies the file
/// to `dest`. Returns the chosen filename on the server.
pub async fn snapshot_export(dest: &Path) -> Result<String> {
    let url = std::env::var("MEMEX_QDRANT_HTTP")
        .unwrap_or_else(|_| "http://localhost:6333".into());
    let client = reqwest::Client::new();
    let create_url = format!("{url}/collections/{COLLECTION}/snapshots");
    let resp: serde_json::Value = client
        .post(&create_url)
        .send()
        .await
        .with_context(|| format!("POST {create_url}"))?
        .error_for_status()?
        .json()
        .await?;
    let name = resp
        .get("result")
        .and_then(|r| r.get("name"))
        .and_then(|n| n.as_str())
        .context("snapshot name missing in response")?
        .to_string();

    let download_url = format!("{url}/collections/{COLLECTION}/snapshots/{name}");
    let bytes = client
        .get(&download_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    tokio::fs::create_dir_all(dest.parent().unwrap_or_else(|| Path::new("."))).await?;
    tokio::fs::write(dest, &bytes).await?;
    Ok(name)
}

pub async fn snapshot_import(src: &Path) -> Result<()> {
    let url = std::env::var("MEMEX_QDRANT_HTTP")
        .unwrap_or_else(|_| "http://localhost:6333".into());
    let bytes = tokio::fs::read(src).await?;
    let client = reqwest::Client::new();
    let upload_url = format!("{url}/collections/{COLLECTION}/snapshots/upload?priority=snapshot");
    let part = reqwest::multipart::Part::bytes(bytes).file_name(
        src.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("memex.snapshot")
            .to_string(),
    );
    let form = reqwest::multipart::Form::new().part("snapshot", part);
    client
        .post(&upload_url)
        .multipart(form)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}
