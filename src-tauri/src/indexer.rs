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
use futures::future::try_join_all;
use once_cell::sync::Lazy;
use qdrant_client::{
    qdrant::{
        point_id::PointIdOptions, vectors_config, ContextInputBuilder, ContextInputPair,
        CreateCollectionBuilder, CreateFieldIndexCollectionBuilder, Distance, FieldType,
        Filter, GetPointsBuilder, PointId, PointStruct, Query,
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

/// Wraps a fastembed `TextEmbedding` (BGE-small-en-v1.5). The model needs
/// `&mut self` to embed (internal ONNX session state), so we serialize access
/// via a `Mutex` and let callers use `&Embedder`.
pub struct Embedder {
    inner: Mutex<TextEmbedding>,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(true),
        )
        .context("loading BGE-small-en-v1.5 fastembed model")?;
        Ok(Self {
            inner: Mutex::new(model),
        })
    }

    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
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

pub fn build_point(session: &Session, vectors_by_name: Vec<(String, Vec<f32>)>) -> PointStruct {
    let id = point_id(&session.session_id);
    let payload = session_payload(session);
    let vec_map: HashMap<String, Vec<f32>> = vectors_by_name.into_iter().collect();
    PointStruct::new(id, vec_map, payload)
}

pub async fn index_session(
    client: &Qdrant,
    embedder: &Embedder,
    session: &Session,
) -> Result<()> {
    let extracts = session_extracts(session);
    let texts: Vec<String> = extracts.iter().map(|(_, t)| t.clone()).collect();
    let vectors = embedder.embed(texts)?;
    let named: Vec<(String, Vec<f32>)> = extracts
        .into_iter()
        .map(|(k, _)| k)
        .zip(vectors.into_iter())
        .collect();
    let point = build_point(session, named);
    client
        .upsert_points(UpsertPointsBuilder::new(COLLECTION, vec![point]).wait(true))
        .await?;
    Ok(())
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
    use indicatif::{ProgressBar, ProgressStyle};
    let pb = ProgressBar::new(sessions.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{wide_bar} {pos}/{len} ({eta}) {msg}")
            .unwrap()
            .progress_chars("=> "),
    );

    // B2: two jsonl files can carry the same `sessionId` (we've seen this in
    // real ~/.claude/projects data). UUID v5(session_id) makes those collide
    // on the Qdrant point id, so the second upsert *silently overwrites*
    // the first. Detect + log + keep the first occurrence so the count
    // we report matches what Qdrant actually stores.
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
}

fn default_weight() -> f32 {
    1.0
}

impl Default for LensWeights {
    fn default() -> Self {
        Self {
            content: 1.0,
            tool: 1.0,
            path: 1.0,
            error: 1.0,
            code: 1.0,
        }
    }
}

impl LensWeights {
    fn iter(&self) -> [(&'static str, f32); 5] {
        [
            ("content", self.content),
            ("tool", self.tool),
            ("path", self.path),
            ("error", self.error),
            ("code", self.code),
        ]
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
    };
    lens_search(client, embedder, query, &weights, limit, 50).await
}

/// **Lens slider** (Plan §3 T3.1).
///
/// Runs one cosine search per named vector whose weight > 0, then performs a
/// weighted score combine in Rust (true weighted blend — not RRF rank fusion).
/// Returns the top `limit` sessions with each session's per-vector contribution
/// in `SearchHit.vector_scores` so the UI can render the lens inspector.
pub async fn lens_search(
    client: &Qdrant,
    embedder: &Embedder,
    query: &str,
    weights: &LensWeights,
    limit: u64,
    per_vector_limit: u64,
) -> Result<Vec<SearchHit>> {
    let vecs = embedder.embed(vec![query.to_string()])?;
    let qvec = vecs.into_iter().next().context("no embedding for query")?;

    let mut combined: HashMap<String, CombinedHit> = HashMap::new();
    let mut payloads: HashMap<String, HashMap<String, qdrant_client::qdrant::Value>> = HashMap::new();

    let mut total_w = 0.0_f32;
    for (_, w) in weights.iter() {
        total_w += w;
    }
    if total_w <= 0.0 {
        return Ok(Vec::new());
    }

    // P1: dispatch one query per non-zero-weight vector in parallel so the
    // wall-clock latency is dominated by the slowest server-side search rather
    // than the sum of all 5. Qdrant handles parallel single-vector queries
    // natively without contention on shared HNSW state.
    let active: Vec<(&'static str, f32)> =
        weights.iter().into_iter().filter(|(_, w)| *w > 0.0).collect();
    let queries = active.iter().map(|(vname, _)| {
        let q: Query = qvec.clone().into();
        let req = QueryPointsBuilder::new(COLLECTION)
            .query(q)
            .using((*vname).to_string())
            .limit(per_vector_limit)
            .with_payload(true);
        async move { client.query(req).await }
    });
    let responses = try_join_all(queries).await?;

    for ((vname, w), res) in active.iter().zip(responses.into_iter()) {
        for p in res.result {
            let sid = match payload_str(&p.payload, "session_id") {
                Some(s) => s,
                None => continue,
            };
            let weighted = p.score * (w / total_w);
            let entry = combined.entry(sid.clone()).or_default();
            entry.combined_score += weighted;
            entry.per_vec.insert((*vname).to_string(), p.score);
            payloads.entry(sid).or_insert(p.payload);
        }
    }

    let mut ranked: Vec<(String, CombinedHit)> = combined.into_iter().collect();
    ranked.sort_by(|a, b| {
        b.1.combined_score
            .partial_cmp(&a.1.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let hits = ranked
        .into_iter()
        .take(limit as usize)
        .map(|(sid, ch)| {
            let p = payloads.get(&sid).cloned().unwrap_or_default();
            SearchHit {
                score: ch.combined_score,
                session_id: sid.clone(),
                project_name: payload_str(&p, "project_name").unwrap_or_default(),
                ai_title: payload_str(&p, "ai_title").unwrap_or_default(),
                start_iso: payload_str(&p, "start_iso").unwrap_or_default(),
                vector_scores: ch.per_vec,
            }
        })
        .collect();
    Ok(hits)
}

#[derive(Default, Debug)]
struct CombinedHit {
    combined_score: f32,
    per_vec: HashMap<String, f32>,
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
            QueryPointsBuilder::new(COLLECTION)
                .query(discover_input)
                .using("content".to_string())
                .limit(limit)
                .with_payload(true),
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
    let resp = client
        .search_matrix_pairs(
            SearchMatrixPointsBuilder::new(COLLECTION)
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
        client
            .get_points(
                GetPointsBuilder::new(COLLECTION, point_ids).with_payload(true),
            )
            .await?
            .result
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
        match crate::parser::scan_dir(&root) {
            Ok(sessions) => compute_insights(&sessions, &nodes, &edges, &matrix2.pairs, &node_project, &pid_to_session),
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
    let res = client
        .query(
            QueryPointsBuilder::new(COLLECTION)
                .query(q)
                .using("error")
                .limit(limit)
                .filter(filter)
                .with_payload(true),
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

/// Fetch a single session's payload (for the inspector pane in the UI).
pub async fn get_session_payload(
    client: &Qdrant,
    session_id: &str,
) -> Result<Option<HashMap<String, qdrant_client::qdrant::Value>>> {
    let pid = PointId {
        point_id_options: Some(PointIdOptions::Uuid(point_id(session_id))),
    };
    let res = client
        .get_points(GetPointsBuilder::new(COLLECTION, vec![pid]).with_payload(true))
        .await?;
    Ok(res.result.into_iter().next().map(|p| p.payload))
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


fn payload_str(
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
