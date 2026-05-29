//! Headless web service for the server-based Memex variant (the single Docker
//! image). Serves the existing `src/` UI over HTTP, exposes the core Qdrant
//! surfaces as a JSON API, and serves MCP over HTTP at `/mcp` (the same tools
//! as the stdio MCP, via `mcp::handle_rpc_value`). Links no Tauri/WebKit.
//!
//! Only compiled under the `web` feature.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::services::ServeDir;

use crate::{crud, indexer, lens, mcp, parser, retrieval, schema, sec};

/// T3.2 (qdrant-improvement-goal.md) — operational observability for the
/// Docker server variant. The all-in-one container ran with no `/metrics`
/// endpoint before this; SREs had no way to alert on rate/latency without
/// log scraping. These eight families let Prometheus scrape /metrics on :8765
/// alongside the existing /api/health.
///
/// Atomic counters are cheap enough that we don't gate them behind a feature
/// flag — every handler bumps the relevant counter unconditionally. The
/// embedder lock-wait histogram is a single Gauge surfaced via `seconds`
/// suffix; we accumulate the total wait time and the count separately so
/// the Prom client can compute an average.
// Issue #16 Stage 2 — exposed `pub` so `mcp::McpState` can carry an
// `Option<Arc<WebMetrics>>` and the new MCP write tool path can increment
// `points_indexed_total` directly. stdio MCP (desktop) leaves it `None`
// (desktop variant has no /metrics endpoint anyway); HTTP MCP (server)
// gets the same Arc that the rest of the web handlers share.
#[derive(Debug, Default)]
pub struct WebMetrics {
    queries_total:          AtomicU64,   // memex_queries_total (search + lens combined)
    recall_polls_total:     AtomicU64,   // memex_recall_polls_total
    // PR #12 REV-1 (Gemini HIGH) — rename: the previous "embedder_lock_waits"
    // was misleading; nothing actually locked a mutex. What we measure now is
    // the wall-clock duration of the embedder-bearing indexer call in each
    // handler — a proxy for embedder throughput plus its Qdrant round-trip.
    // Stored in ms internally; emitted as seconds in the Prom exposition.
    embedder_call_ms_sum:   AtomicU64,   // memex_embedder_call_seconds_sum
    embedder_call_count:    AtomicU64,   // memex_embedder_call_seconds_count
    snapshot_bytes:         AtomicU64,   // memex_snapshot_bytes (last successful export size)
    points_indexed_total:   AtomicU64,   // memex_points_indexed_total
    errors_recalled_total:  AtomicU64,   // memex_errors_recalled_total (hits surfaced by recall lane)
    mcp_calls_total:        AtomicU64,   // memex_mcp_calls_total (informational)
    process_start:          std::sync::OnceLock<Instant>, // for memex_process_uptime_seconds
}

impl WebMetrics {
    pub fn mark_query(&self)        { self.queries_total.fetch_add(1, Ordering::Relaxed); }
    pub fn mark_recall_poll(&self)  { self.recall_polls_total.fetch_add(1, Ordering::Relaxed); }
    /// PR #12 REV-2 (Gemini medium) — bulk add instead of looping `fetch_add(1)`
    /// per hit. One atomic op per call, regardless of how many hits the recall
    /// lane surfaced.
    pub fn mark_recall_hits(&self, n: u64) {
        if n > 0 {
            self.errors_recalled_total.fetch_add(n, Ordering::Relaxed);
        }
    }
    pub fn mark_mcp_call(&self)     { self.mcp_calls_total.fetch_add(1, Ordering::Relaxed); }
    /// Issue #16 Stage 2 — called from the MCP write-tool path (`refresh_session_enrich`)
    /// when the tool successfully sets payload on N points. The desktop stdio MCP
    /// path holds `None` for metrics and skips this; HTTP MCP wires it through.
    pub fn mark_indexed(&self, n: u64) { self.points_indexed_total.fetch_add(n, Ordering::Relaxed); }
    /// PR #12 REV-1 (Gemini HIGH) — actually instrument the family that
    /// /metrics exposes. Call this around any embedder-bearing handler so the
    /// summary buckets accumulate real samples (not the zero-forever values
    /// gemini-code-assist flagged on the first review pass).
    pub fn record_embedder_call(&self, elapsed: std::time::Duration) {
        let ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
        self.embedder_call_ms_sum.fetch_add(ms, Ordering::Relaxed);
        self.embedder_call_count.fetch_add(1, Ordering::Relaxed);
    }
    pub fn started_at(&self) -> Instant {
        *self.process_start.get_or_init(Instant::now)
    }
}

#[derive(Clone)]
struct WebState {
    qdrant: Arc<qdrant_client::Qdrant>,
    embedder: Arc<indexer::Embedder>,
    mcp: mcp::SharedMcpState,
    ui_dir: PathBuf,
    metrics: Arc<WebMetrics>,
}

/// Filesystem root the browser UI scans for sessions (list_sessions, replay,
/// predict — these re-parse source `.jsonl` through the `sec` sandbox, so it
/// must be under `~/.claude/projects` or `~/.codex/sessions`). Defaults to the
/// bundled sample corpus for local dev; the Docker image sets MEMEX_SCAN_ROOT
/// to a path under $HOME/.claude/projects so replay/predict work too.
fn web_scan_root() -> PathBuf {
    std::env::var("MEMEX_SCAN_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("examples/sample-corpus"))
}

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

fn err500<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
}

// PR6-D — input clamps. Unbounded `limit` / `sample` / `per_point` from the
// query string let a single request ask Qdrant for an arbitrarily large result
// set / distance matrix → a cheap memory + CPU DoS. Clamp every user-supplied
// count to a sane ceiling at the handler boundary.
const MAX_LIMIT: u64 = 200;
const MAX_SAMPLE: u32 = 500;
const MAX_PER_POINT: u32 = 20;

#[inline]
fn clamp_limit(n: u64) -> u64 {
    n.clamp(1, MAX_LIMIT)
}
#[inline]
fn clamp_sample(n: u32) -> u32 {
    n.clamp(1, MAX_SAMPLE)
}
#[inline]
fn clamp_per_point(n: u32) -> u32 {
    n.clamp(1, MAX_PER_POINT)
}

/// Start the web service. Qdrant must be reachable (the Docker entrypoint waits
/// for `/readyz` before launching this). The embedder (BGE-small) loads once at
/// startup so requests are warm.
pub async fn serve(port: u16, ui_dir: PathBuf) -> Result<()> {
    eprintln!("[memex-web] connecting to Qdrant…");
    let qdrant = Arc::new(
        indexer::connect()
            .await
            .context("connecting to Qdrant (is it running? set MEMEX_QDRANT_URL)")?,
    );
    // Best-effort: ensure both collections exist so first query/index works.
    let _ = crud::ensure_collection_v3(&qdrant).await;
    let _ = indexer::ensure_collection(&qdrant).await;

    eprintln!("[memex-web] loading embedder (BGE-small, first run downloads ~130MB)…");
    let embedder = Arc::new(indexer::Embedder::new().context("initializing embedder")?);

    // Optionally index the scan root on startup so the browser UI shows data
    // immediately (best-effort; logged on failure, never blocks serving).
    if std::env::var("MEMEX_WEB_AUTOINDEX")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false)
    {
        let root = web_scan_root();
        eprintln!("[memex-web] auto-indexing {} …", root.display());
        let sessions = parser::scan_dir(&root).unwrap_or_default();
        match indexer::bulk_index_arc(&qdrant, embedder.clone(), &sessions).await {
            Ok(r) => eprintln!("[memex-web] auto-indexed {}/{} session(s)", r.indexed, sessions.len()),
            Err(e) => eprintln!("[memex-web] auto-index skipped: {e:#}"),
        }
    }

    let metrics = Arc::new(WebMetrics::default());
    // Stamp the process_start lazy-init now so /metrics uptime starts here,
    // not on the first request.
    let _ = metrics.started_at();

    let state = WebState {
        qdrant,
        embedder,
        // Issue #16 Stage 2 — wire the same metrics Arc into the shared MCP
        // state so the `refresh_session_enrich` write tool can flip
        // `memex_points_indexed_total` off zero on this transport.
        mcp: mcp::new_shared_state_with_metrics(metrics.clone()),
        ui_dir: ui_dir.clone(),
        metrics,
    };

    let api = Router::new()
        .route("/api/health", get(health))
        // T3.2 — Prometheus exposition for the Docker server variant.
        .route("/metrics", get(metrics_handler))
        // PR #12 REV-14 (self-review) — closes the snapshot_bytes orphan
        // metric. Posts to indexer::snapshot_export, reads the resulting
        // file size, updates WebMetrics::snapshot_bytes, returns
        // { name, bytes } so the operator can verify the export landed.
        .route("/api/snapshot/export", post(snapshot_export_handler))
        .route("/api/search", get(search))
        .route("/api/lens", post(lens))
        .route("/api/recall", get(recall))
        .route("/api/topology", get(topology))
        .route("/api/mix", post(mix))
        .route("/api/index", post(index_path))
        // Generic Tauri-command bridge: the browser UI's __TAURI__ shim posts
        // here so the existing frontend works unchanged over HTTP.
        .route("/api/invoke/{cmd}", post(invoke_handler))
        // PR6-E: only `POST /mcp` (JSON-RPC) is kept — that's what
        // `claude mcp add --transport http` uses. The previous `GET /mcp` SSE
        // route was a non-compliant readiness ping no client consumed; it's
        // removed along with its handler/imports.
        .route("/mcp", post(mcp_post));

    let app = Router::new()
        .merge(api)
        // Serve the html entrypoints with the __TAURI__ fetch shim injected
        // (web only). Both index.html and dashboard.html drive the backend via
        // `invoke()`, so both need the shim; everything else (css/js/assets)
        // falls through to the static dir below.
        .route("/", get(index_html))
        .route("/index.html", get(index_html))
        .route("/dashboard.html", get(dashboard_html))
        .fallback_service(ServeDir::new(&ui_dir).append_index_html_on_directories(true))
        // PR6-A: NO `CorsLayer::permissive()`. The browser UI is served
        // same-origin from this very server, so it needs no CORS headers;
        // `permissive()` (Access-Control-Allow-Origin: *) would let ANY website
        // the user visits read the entire indexed corpus via fetch(). Removed.
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    eprintln!(
        "[memex-web] listening on http://{addr}  ·  UI: {}  ·  MCP (HTTP): http://{addr}/mcp",
        ui_dir.display()
    );
    axum::serve(listener, app)
        .await
        .context("axum server error")?;
    Ok(())
}

// ---- JSON API ------------------------------------------------------------

async fn health(State(s): State<WebState>) -> Json<Value> {
    let qdrant_ok = s.qdrant.health_check().await.is_ok();
    Json(json!({
        "status": "ok",
        "service": "memex-web",
        "qdrant": qdrant_ok,
    }))
}

// ---- /metrics (Prometheus exposition format 0.0.4) -----------------------
//
// T3.2 (qdrant-improvement-goal.md) — eight metric families let Prometheus
// scrape the all-in-one container alongside `/api/health`. Output uses the
// canonical `text/plain; version=0.0.4; charset=utf-8` content type so
// `prometheus-client_python` and the official Go `prometheus_client` both
// parse it without a custom format handler.
//
// All values come from the atomic counters in `WebMetrics`. The embedder
// call family is a Summary-style pair (sum_seconds + count) populated by
// each search/lens/recall handler via record_embedder_call(); the
// process_uptime family is computed from `Instant::now() - started_at`.
//
// PR #12 REV-1 (Gemini HIGH) — the previous "lock_waits" family advertised
// a measurement that wasn't actually taken. The family is now renamed
// `embedder_call_seconds` (sum + count) and is properly instrumented in
// each embedder-bearing handler — the summary buckets accumulate real
// samples.

async fn metrics_handler(State(s): State<WebState>) -> Response {
    let m = &*s.metrics;
    let started = m.started_at();
    let uptime_secs = started.elapsed().as_secs_f64();

    let queries          = m.queries_total.load(Ordering::Relaxed);
    let recall_polls     = m.recall_polls_total.load(Ordering::Relaxed);
    let call_ms_sum      = m.embedder_call_ms_sum.load(Ordering::Relaxed);
    let call_count       = m.embedder_call_count.load(Ordering::Relaxed);
    let snapshot_bytes   = m.snapshot_bytes.load(Ordering::Relaxed);
    let points_indexed   = m.points_indexed_total.load(Ordering::Relaxed);
    let errors_recalled  = m.errors_recalled_total.load(Ordering::Relaxed);
    let mcp_calls        = m.mcp_calls_total.load(Ordering::Relaxed);

    // Prometheus 0.0.4 text exposition. One HELP + TYPE comment per family,
    // single metric line per family for counters. Six families minimum per
    // SOT §3 acceptance ("Prometheus exposition with >=6 metric families");
    // we ship eight to cover the SLO axes the operator most likely wants.
    let body = format!(
"# HELP memex_queries_total Total search + lens queries served by the web variant.
# TYPE memex_queries_total counter
memex_queries_total {queries}

# HELP memex_recall_polls_total Total proactive-recall poll cycles.
# TYPE memex_recall_polls_total counter
memex_recall_polls_total {recall_polls}

# HELP memex_errors_recalled_total Total recall hits surfaced to the UI / agent.
# TYPE memex_errors_recalled_total counter
memex_errors_recalled_total {errors_recalled}

# HELP memex_points_indexed_total Total session points indexed via /api/index or MCP.
# TYPE memex_points_indexed_total counter
memex_points_indexed_total {points_indexed}

# HELP memex_snapshot_bytes Size in bytes of the most recent successful Qdrant snapshot export (0 if none).
# TYPE memex_snapshot_bytes gauge
memex_snapshot_bytes {snapshot_bytes}

# HELP memex_embedder_call_seconds Wall-clock duration of each embedder-bearing search/lens/recall call (summary).
# TYPE memex_embedder_call_seconds summary
memex_embedder_call_seconds_sum {call_seconds_sum}
memex_embedder_call_seconds_count {call_count}

# HELP memex_mcp_calls_total Total MCP JSON-RPC calls received on /mcp.
# TYPE memex_mcp_calls_total counter
memex_mcp_calls_total {mcp_calls}

# HELP memex_process_uptime_seconds Seconds since this memex-web process started.
# TYPE memex_process_uptime_seconds gauge
memex_process_uptime_seconds {uptime_secs}
",
        queries           = queries,
        recall_polls      = recall_polls,
        errors_recalled   = errors_recalled,
        points_indexed    = points_indexed,
        snapshot_bytes    = snapshot_bytes,
        call_seconds_sum  = (call_ms_sum as f64) / 1000.0,
        call_count        = call_count,
        mcp_calls         = mcp_calls,
        uptime_secs       = uptime_secs,
    );

    let mut response = body.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        "text/plain; version=0.0.4; charset=utf-8".parse().unwrap(),
    );
    response
}

#[derive(Deserialize)]
struct SearchQ {
    q: String,
    #[serde(default = "default_limit")]
    limit: u64,
}
fn default_limit() -> u64 {
    10
}

async fn search(State(s): State<WebState>, Query(q): Query<SearchQ>) -> ApiResult {
    s.metrics.mark_query();
    let limit = clamp_limit(q.limit);
    let t = Instant::now();
    let hits = indexer::search_content(s.qdrant.as_ref(), s.embedder.as_ref(), &q.q, limit)
        .await
        .map_err(err500)?;
    s.metrics.record_embedder_call(t.elapsed());
    Ok(Json(json!({ "query": q.q, "hits": hits })))
}

#[derive(Deserialize)]
struct LensReq {
    query: String,
    weights: Option<indexer::LensWeights>,
    #[serde(default = "default_limit")]
    limit: u64,
}

async fn lens(State(s): State<WebState>, Json(body): Json<LensReq>) -> ApiResult {
    s.metrics.mark_query();
    // Issue #15 — LensWeights now lives in `crate::lens` (re-exported via
    // `indexer::LensWeights`). `Default::default()` returns the canonical
    // values: 5×1.0 + content_late 0.25 + diversity None + fusion Formula.
    let weights = body.weights.unwrap_or_default();
    let t = Instant::now();
    let hits = indexer::lens_search(
        s.qdrant.as_ref(),
        s.embedder.as_ref(),
        &body.query,
        &weights,
        clamp_limit(body.limit),
        60,
    )
    .await
    .map_err(err500)?;
    s.metrics.record_embedder_call(t.elapsed());
    Ok(Json(json!({ "query": body.query, "hits": hits })))
}

async fn recall(State(s): State<WebState>, Query(q): Query<SearchQ>) -> ApiResult {
    s.metrics.mark_recall_poll();
    let t = Instant::now();
    let hits = indexer::recall(s.qdrant.as_ref(), s.embedder.as_ref(), &q.q, clamp_limit(q.limit))
        .await
        .map_err(err500)?;
    s.metrics.record_embedder_call(t.elapsed());
    // PR #12 REV-2 (Gemini medium) — one atomic add for the whole batch.
    // The previous `for _ in 0..hits.len() { mark_recall_hit() }` issued N
    // independent `fetch_add(1)` ops, each fighting cache coherence on the
    // counter; mark_recall_hits(n) does a single `fetch_add(n, Relaxed)`.
    s.metrics.mark_recall_hits(hits.len() as u64);
    Ok(Json(json!({ "error_text": q.q, "hits": hits })))
}

#[derive(Deserialize)]
struct TopoQ {
    #[serde(default = "default_sample")]
    sample: u32,
    #[serde(default = "default_per_point")]
    per_point: u32,
}
fn default_sample() -> u32 {
    80
}
fn default_per_point() -> u32 {
    5
}

async fn topology(State(s): State<WebState>, Query(q): Query<TopoQ>) -> ApiResult {
    let topo = indexer::topology(
        s.qdrant.as_ref(),
        clamp_sample(q.sample),
        clamp_per_point(q.per_point),
        None,
    )
    .await
    .map_err(err500)?;
    Ok(Json(serde_json::to_value(topo).map_err(err500)?))
}

#[derive(Deserialize)]
struct MixReq {
    #[serde(default)]
    pos: Vec<String>,
    #[serde(default)]
    neg: Vec<String>,
    #[serde(default = "default_limit")]
    limit: u64,
}

async fn mix(State(s): State<WebState>, Json(body): Json<MixReq>) -> ApiResult {
    if body.pos.is_empty() && body.neg.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "provide at least one pos or neg session id".into()));
    }
    let hits = indexer::mix_match(s.qdrant.as_ref(), &body.pos, &body.neg, clamp_limit(body.limit))
        .await
        .map_err(err500)?;
    Ok(Json(json!({ "hits": hits })))
}

#[derive(Deserialize)]
struct IndexReq {
    /// Directory of Claude-format `*.jsonl` sessions (e.g. examples/sample-corpus).
    path: String,
}

async fn index_path(State(s): State<WebState>, Json(body): Json<IndexReq>) -> ApiResult {
    // PR6-B: the caller-supplied path is re-scanned + each session's source
    // jsonl re-parsed, so it MUST live inside the `sec` sandbox
    // (~/.claude/projects or ~/.codex/sessions). Reject anything else with 403
    // before touching the filesystem — otherwise a request could index (and
    // thereby exfiltrate via search) arbitrary directories.
    let path = sec::validate_session_path(std::path::Path::new(&body.path))
        .map_err(|e| (StatusCode::FORBIDDEN, format!("path rejected: {e:#}")))?;
    let sessions = tokio::task::spawn_blocking(move || parser::scan_dir(&path))
        .await
        .map_err(|e| err500(format!("scan task panicked: {e}")))?
        .map_err(err500)?;
    let total = sessions.len();
    crud::ensure_collection_v3(s.qdrant.as_ref())
        .await
        .map_err(err500)?;
    indexer::ensure_collection(s.qdrant.as_ref())
        .await
        .map_err(err500)?;
    let report = indexer::bulk_index_arc(s.qdrant.as_ref(), s.embedder.clone(), &sessions)
        .await
        .map_err(err500)?;
    s.metrics.mark_indexed(report.indexed as u64);
    Ok(Json(json!({
        "path": body.path,
        "total": total,
        "indexed": report.indexed,
        "duplicates_skipped": report.duplicates_skipped,
        "errors": report.errors,
    })))
}

// ---- Snapshot export (PR #12 REV-14 self-review) -------------------------
//
// Closes the `memex_snapshot_bytes` orphan metric. Before this route existed,
// the Prometheus gauge was always 0 because nothing in the web variant ever
// triggered a snapshot. Now an operator can POST to /api/snapshot/export,
// the indexer's snapshot endpoint is exercised, and the resulting file's
// size on disk lands in the gauge so Prometheus can alert on absent backups.
//
// The snapshot lives under MEMEX_SNAPSHOT_DIR (or the user's HOME by default)
// — the existing indexer::snapshot_export resolves the path; we just measure
// what it produced.

#[derive(Deserialize, Default)]
#[serde(default)]
struct SnapshotReq {
    /// Optional destination directory. Defaults to $MEMEX_SNAPSHOT_DIR or
    /// $HOME — the indexer applies the same resolution as the desktop CLI.
    dir: Option<String>,
}

async fn snapshot_export_handler(
    State(s): State<WebState>,
    Json(body): Json<SnapshotReq>,
) -> ApiResult {
    use std::path::PathBuf;
    // indexer::snapshot_export expects a FILE path, not a directory — it
    // writes the snapshot bytes directly to `dest`. Mirror what
    // commands::snapshot_export_default does on the desktop side: resolve a
    // directory (caller-supplied or env MEMEX_SNAPSHOT_DIR or HOME), append
    // a timestamped filename, then hand that file path to the indexer.
    let dir: PathBuf = match body.dir {
        Some(d) => PathBuf::from(d),
        None => std::env::var("MEMEX_SNAPSHOT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("/tmp"))
            }),
    };
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let dest = dir.join(format!("memex-snapshot-{ts}.snapshot"));
    let name = indexer::snapshot_export(&dest).await.map_err(err500)?;
    // Measure the resulting file size and update the Prometheus gauge.
    let bytes = tokio::fs::metadata(&dest).await.map(|m| m.len()).unwrap_or(0);
    s.metrics
        .snapshot_bytes
        .store(bytes, Ordering::Relaxed);
    Ok(Json(json!({
        "name": name,
        "path": dest.to_string_lossy(),
        "bytes": bytes,
    })))
}

// ---- MCP over HTTP -------------------------------------------------------

/// POST /mcp — JSON-RPC 2.0 request in, response out. Same tools as stdio MCP.
/// Register from Claude CLI:  `claude mcp add --transport http memex-web http://localhost:<port>/mcp`
async fn mcp_post(State(s): State<WebState>, Json(body): Json<Value>) -> impl axum::response::IntoResponse {
    s.metrics.mark_mcp_call();
    match mcp::handle_rpc_value(&s.mcp, body).await {
        Some(resp) => (StatusCode::OK, Json(resp)).into_response(),
        // Notification (no id) — JSON-RPC says no response body.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

// ---- Generic Tauri-command bridge (browser UI <-> HTTP) ------------------
// The browser UI keeps calling `invoke("cmd", args)`; the injected shim posts
// those to /api/invoke/{cmd}, and this dispatcher mirrors the GUI's
// invoke_handler so the existing frontend works unchanged over HTTP.

async fn invoke_handler(
    State(s): State<WebState>,
    Path(cmd): Path<String>,
    Json(args): Json<Value>,
) -> ApiResult {
    dispatch_invoke(&s, &cmd, args).await
}

fn a_str(args: &Value, k: &str) -> Option<String> {
    args.get(k).and_then(|v| v.as_str()).map(str::to_string)
}
fn a_u64(args: &Value, k: &str) -> Option<u64> {
    args.get(k).and_then(Value::as_u64)
}
fn a_u32(args: &Value, k: &str) -> Option<u32> {
    args.get(k).and_then(Value::as_u64).map(|n| n as u32)
}
fn a_usize(args: &Value, k: &str) -> Option<usize> {
    args.get(k).and_then(Value::as_u64).map(|n| n as usize)
}
fn a_strs(args: &Value, k: &str) -> Vec<String> {
    args.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}
fn de_opt<T: serde::de::DeserializeOwned>(args: &Value, k: &str) -> Option<T> {
    args.get(k).cloned().and_then(|v| serde_json::from_value(v).ok())
}
fn de_def<T: serde::de::DeserializeOwned + Default>(args: &Value, k: &str) -> T {
    de_opt(args, k).unwrap_or_default()
}
fn ok_json<T: serde::Serialize>(v: T) -> ApiResult {
    Ok(Json(serde_json::to_value(v).map_err(err500)?))
}

async fn dispatch_invoke(s: &WebState, cmd: &str, args: Value) -> ApiResult {
    let q = s.qdrant.as_ref();
    let e = s.embedder.as_ref();
    match cmd {
        "lens_search" => {
            let w: indexer::LensWeights = de_def(&args, "weights");
            ok_json(
                indexer::lens_search(q, e, &a_str(&args, "query").unwrap_or_default(), &w, a_u64(&args, "limit").unwrap_or(20), 60)
                    .await
                    .map_err(err500)?,
            )
        }
        "lens_search_v2" => {
            let w: lens::LensWeights = de_def(&args, "weights");
            ok_json(
                lens::lens_search_v2(q, e, &a_str(&args, "query").unwrap_or_default(), &w, a_u64(&args, "limit").unwrap_or(20))
                    .await
                    .map_err(err500)?,
            )
        }
        "lens_search_grouped" => ok_json(
            retrieval::lens_search_grouped(q, e, &a_str(&args, "query").unwrap_or_default(), de_opt(&args, "groupBy"), a_u64(&args, "limit").unwrap_or(20))
                .await
                .map_err(err500)?,
        ),
        "mix_match" => ok_json(
            indexer::mix_match(q, &a_strs(&args, "positive"), &a_strs(&args, "negative"), a_u64(&args, "limit").unwrap_or(20))
                .await
                .map_err(err500)?,
        ),
        "mix_match_with_pairs" => {
            let pairs: Vec<retrieval::ContextPair> = de_def(&args, "pairs");
            ok_json(
                retrieval::mix_match_with_pairs(q, &a_str(&args, "targetSessionId").unwrap_or_default(), &pairs, a_u64(&args, "limit").unwrap_or(20))
                    .await
                    .map_err(err500)?,
            )
        }
        "relevance_feedback" => ok_json(
            retrieval::relevance_feedback(q, e, &a_strs(&args, "positiveIds"), &a_strs(&args, "negativeIds"), &a_str(&args, "previousQuery").unwrap_or_default(), a_u64(&args, "limit").unwrap_or(20))
                .await
                .map_err(err500)?,
        ),
        "topology" => ok_json(
            indexer::topology(q, a_u32(&args, "sample").unwrap_or(80), a_u32(&args, "perPoint").unwrap_or(5), None)
                .await
                .map_err(err500)?,
        ),
        "recall" => ok_json(
            indexer::recall(q, e, &a_str(&args, "errorText").unwrap_or_default(), a_u64(&args, "limit").unwrap_or(5))
                .await
                .map_err(err500)?,
        ),
        "predict_next_actions" => ok_json(
            indexer::predict_next_actions(q, e, &a_str(&args, "sessionId").unwrap_or_default(), a_usize(&args, "lastNTurns").unwrap_or(3), a_usize(&args, "horizon").unwrap_or(3), a_u64(&args, "neighbors").unwrap_or(8))
                .await
                .map_err(err500)?,
        ),
        "get_session" => {
            let sid = a_str(&args, "sessionId").unwrap_or_default();
            match indexer::get_session_payload(q, &sid).await.map_err(err500)? {
                None => ok_json(Value::Null),
                Some(p) => {
                    let mut out = serde_json::Map::new();
                    for (k, v) in p {
                        out.insert(k, qdrant_value_to_json(v));
                    }
                    ok_json(Value::Object(out))
                }
            }
        }
        "get_session_turns" => {
            let sid = a_str(&args, "sessionId").unwrap_or_default();
            let payload = indexer::get_session_payload(q, &sid)
                .await
                .map_err(err500)?
                .ok_or((StatusCode::NOT_FOUND, format!("session {sid} not in index")))?;
            let source = payload
                .get("source_path")
                .and_then(|v| v.kind.as_ref())
                .and_then(|k| match k {
                    qdrant_client::qdrant::value::Kind::StringValue(s) => Some(s.clone()),
                    _ => None,
                })
                .ok_or((
                    StatusCode::BAD_REQUEST,
                    "session payload missing source_path".to_string(),
                ))?;
            let validated =
                sec::validate_session_path(std::path::Path::new(&source)).map_err(err500)?;
            let source_agent = indexer::payload_str(&payload, "source_agent")
                .unwrap_or_else(|| "claude_code".to_string());
            ok_json(
                crate::session_roots::parse_session_routed(&source_agent, &validated)
                    .map_err(err500)?,
            )
        }
        "list_sessions" => {
            // PR6-C: a user-supplied `path` is sandbox-validated (403 on
            // reject); absent → the default scan root (trusted, set by the
            // operator via MEMEX_SCAN_ROOT).
            let root = match a_str(&args, "path") {
                Some(p) => sec::validate_session_path(std::path::Path::new(&p))
                    .map_err(|e| (StatusCode::FORBIDDEN, format!("path rejected: {e:#}")))?,
                None => web_scan_root(),
            };
            let limit = a_usize(&args, "limit").unwrap_or(60);
            let mut sessions = tokio::task::spawn_blocking(move || parser::scan_dir(&root))
                .await
                .map_err(|e| err500(format!("scan panicked: {e}")))?
                .map_err(err500)?;
            sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));
            let out: Vec<crate::summary::SessionSummary> = sessions.into_iter().take(limit).map(Into::into).collect();
            ok_json(out)
        }
        "list_sessions_ordered" => ok_json(
            retrieval::list_sessions_ordered(q, de_opt(&args, "orderBy"), None, a_u32(&args, "limit").unwrap_or(60))
                .await
                .map_err(err500)?,
        ),
        "collection_info" => {
            let info = q.collection_info(schema::COLLECTION_V3).await.map_err(err500)?;
            let r = info.result.unwrap_or_default();
            ok_json(json!({
                "collection": schema::COLLECTION_V3,
                "points_count": r.points_count.unwrap_or(0),
                "indexed_vectors_count": r.indexed_vectors_count.unwrap_or(0),
                "status": r.status,
                "segments_count": r.segments_count,
            }))
        }
        "refresh_index" => {
            // PR6-C: same sandbox-validation as list_sessions for a
            // user-supplied path; default scan root otherwise.
            let root = match a_str(&args, "path") {
                Some(p) => sec::validate_session_path(std::path::Path::new(&p))
                    .map_err(|e| (StatusCode::FORBIDDEN, format!("path rejected: {e:#}")))?,
                None => web_scan_root(),
            };
            let sessions = tokio::task::spawn_blocking(move || parser::scan_dir(&root))
                .await
                .map_err(|e| err500(format!("scan panicked: {e}")))?
                .map_err(err500)?;
            let total = sessions.len();
            crud::ensure_collection_v3(q).await.map_err(err500)?;
            indexer::ensure_collection(q).await.map_err(err500)?;
            let report = indexer::bulk_index_arc(q, s.embedder.clone(), &sessions).await.map_err(err500)?;
            ok_json(json!({
                "indexed": report.indexed,
                "duplicates_skipped": report.duplicates_skipped,
                "errors": report.errors,
                "total_scanned": total,
            }))
        }
        // A static server corpus has no live sessions changing under it, so
        // proactive-recall polling correctly returns empty (not a stub).
        "tail_recent_errors" => ok_json(Vec::<Value>::new()),
        // Dashboard activity-heatmap source. Reads ~/.claude/history.jsonl;
        // when absent (the usual case for a server) the parser returns empty
        // stats rather than erroring, so the heatmap just shows no prompt data.
        "prompt_history_stats" => {
            let path = a_str(&args, "path").map(PathBuf::from).unwrap_or_else(|| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                PathBuf::from(home).join(".claude").join("history.jsonl")
            });
            ok_json(
                tokio::task::spawn_blocking(move || parser::read_prompt_history_stats(&path))
                    .await
                    .map_err(|e| err500(format!("history stats task panicked: {e}")))?
                    .map_err(err500)?,
            )
        }
        // Main UI "Snapshot" button: it prompts for a destination path (works
        // in a browser) and passes it here. Mirrors the Tauri command, writing
        // the Qdrant snapshot to that path inside the container.
        "snapshot_export" => {
            let path = a_str(&args, "path")
                .ok_or((StatusCode::BAD_REQUEST, "snapshot_export requires 'path'".to_string()))?;
            let name = indexer::snapshot_export(&PathBuf::from(&path)).await.map_err(err500)?;
            ok_json(name)
        }
        "snapshot_export_default" => {
            let dir = std::env::var("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
            let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
            let path = dir.join(format!("memex-snapshot-{ts}.snapshot"));
            let name = indexer::snapshot_export(&path).await.map_err(err500)?;
            ok_json(json!({ "name": name, "path": path.display().to_string() }))
        }
        // Companion "Cold Start Killer" — the headline #7 surface. The browser
        // UI (Companion modal / `memex://companion`) invokes this; without a web
        // dispatcher arm it 404'd, leaving the feature broken in the Docker
        // variant (frontend gap analysis). Mirrors the Tauri command; the web
        // server warms the embedder at startup, so the non-lazy path is fine.
        "compose_memory_primer" => {
            let cwd_path = a_str(&args, "cwd").filter(|s| !s.is_empty()).map(PathBuf::from);
            let resolved = crate::companion::resolve_cwd_arg(cwd_path.as_deref()).map_err(err500)?;
            let limit = a_usize(&args, "limit").unwrap_or(8);
            ok_json(
                crate::companion::compose_memory_primer(q, e, &resolved, limit)
                    .await
                    .map_err(err500)?,
            )
        }
        other => Err((StatusCode::NOT_FOUND, format!("unknown command: {other}"))),
    }
}

/// Read a UI html entrypoint and inject the __TAURI__ fetch shim right after
/// `<head>` so the existing browser frontend's `invoke()`/`event.listen()`
/// calls work over HTTP. Used for *every* html page the web server owns
/// (index.html AND dashboard.html) — both call `window.__TAURI__.core.invoke`,
/// so both need the shim or the page renders but never loads data. The Tauri
/// desktop app loads these files directly with the real runtime and is
/// unaffected (it never hits these routes).
async fn html_with_shim(ui_dir: &std::path::Path, file: &str) -> Response {
    let path = ui_dir.join(file);
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => {
            let shim = format!("<script>{SHIM_JS}</script>");
            let injected = match html.find("<head>") {
                Some(pos) => {
                    let mut h = html.clone();
                    h.insert_str(pos + "<head>".len(), &shim);
                    h
                }
                None => format!("{shim}{html}"),
            };
            Html(injected).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, format!("{file} not found")).into_response(),
    }
}

async fn index_html(State(s): State<WebState>) -> Response {
    html_with_shim(&s.ui_dir, "index.html").await
}

async fn dashboard_html(State(s): State<WebState>) -> Response {
    html_with_shim(&s.ui_dir, "dashboard.html").await
}

const SHIM_JS: &str = r#"
(function(){
  // The Tauri desktop app ships a bundled window icon; the web server serves a
  // static dir without one, so the browser would 404 on /favicon.ico. Inject a
  // brand favicon (rounded dark tile + accent dot) so the console stays clean.
  try {
    if (document.head && !document.querySelector('link[rel="icon"]')) {
      var fav = document.createElement('link');
      fav.rel = 'icon';
      fav.type = 'image/svg+xml';
      fav.href = "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><rect width='32' height='32' rx='7' fill='%230f0f12'/><circle cx='16' cy='16' r='7' fill='%230a84ff'/></svg>";
      document.head.appendChild(fav);
    }
  } catch (e) { /* non-fatal */ }

  if (window.__TAURI__ && window.__TAURI__.core && typeof window.__TAURI__.core.invoke === 'function') return;
  async function invoke(cmd, args){
    // Tauri plugin IPC (deep-link, notification, opener, …) is delivered by the
    // native runtime and has no server-side equivalent in the web variant.
    // Resolve to null instead of POSTing to a command that would 404.
    if (typeof cmd === 'string' && cmd.indexOf('plugin:') === 0) return null;
    const res = await fetch('/api/invoke/'+cmd, {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify(args||{})});
    if(!res.ok){ throw new Error('invoke '+cmd+' failed: '+res.status+' '+(await res.text())); }
    return await res.json();
  }
  const noop = function(){};
  const event = {
    listen: async function(){ return noop; },
    once: async function(){ return noop; },
    emit: async function(){},
  };
  window.__TAURI__ = Object.assign(window.__TAURI__||{}, { core: { invoke: invoke }, invoke: invoke, event: event });
})();
"#;

fn qdrant_value_to_json(v: qdrant_client::qdrant::Value) -> Value {
    use qdrant_client::qdrant::value::Kind;
    match v.kind {
        Some(Kind::NullValue(_)) | None => Value::Null,
        Some(Kind::BoolValue(b)) => Value::Bool(b),
        Some(Kind::IntegerValue(i)) => Value::Number(i.into()),
        Some(Kind::DoubleValue(d)) => serde_json::Number::from_f64(d).map(Value::Number).unwrap_or(Value::Null),
        Some(Kind::StringValue(s)) => Value::String(s),
        Some(Kind::ListValue(l)) => Value::Array(l.values.into_iter().map(qdrant_value_to_json).collect()),
        Some(Kind::StructValue(st)) => {
            let mut m = serde_json::Map::new();
            for (k, vv) in st.fields {
                m.insert(k, qdrant_value_to_json(vv));
            }
            Value::Object(m)
        }
    }
}
