//! Headless web service for the server-based Memex variant (the single Docker
//! image). Serves the existing `src/` UI over HTTP, exposes the core Qdrant
//! surfaces as a JSON API, and serves MCP over HTTP at `/mcp` (the same tools
//! as the stdio MCP, via `mcp::handle_rpc_value`). Links no Tauri/WebKit.
//!
//! Only compiled under the `web` feature.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use crate::{crud, indexer, mcp, parser};

#[derive(Clone)]
struct WebState {
    qdrant: Arc<qdrant_client::Qdrant>,
    embedder: Arc<indexer::Embedder>,
    mcp: mcp::SharedMcpState,
}

type ApiResult = Result<Json<Value>, (StatusCode, String)>;

fn err500<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
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

    let state = WebState {
        qdrant,
        embedder,
        mcp: mcp::new_shared_state(),
    };

    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/search", get(search))
        .route("/api/lens", post(lens))
        .route("/api/recall", get(recall))
        .route("/api/topology", get(topology))
        .route("/api/mix", post(mix))
        .route("/api/index", post(index_path))
        .route("/mcp", post(mcp_post).get(mcp_sse));

    let app = Router::new()
        .merge(api)
        .fallback_service(ServeDir::new(&ui_dir).append_index_html_on_directories(true))
        .layer(CorsLayer::permissive())
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
    let hits = indexer::search_content(s.qdrant.as_ref(), s.embedder.as_ref(), &q.q, q.limit)
        .await
        .map_err(err500)?;
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
    let weights = body.weights.unwrap_or(indexer::LensWeights {
        content: 1.0,
        tool: 1.0,
        path: 1.0,
        error: 1.0,
        code: 1.0,
        content_late: 0.0,
    });
    let hits = indexer::lens_search(
        s.qdrant.as_ref(),
        s.embedder.as_ref(),
        &body.query,
        &weights,
        body.limit,
        60,
    )
    .await
    .map_err(err500)?;
    Ok(Json(json!({ "query": body.query, "hits": hits })))
}

async fn recall(State(s): State<WebState>, Query(q): Query<SearchQ>) -> ApiResult {
    let hits = indexer::recall(s.qdrant.as_ref(), s.embedder.as_ref(), &q.q, q.limit)
        .await
        .map_err(err500)?;
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
    let topo = indexer::topology(s.qdrant.as_ref(), q.sample, q.per_point, None)
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
    let hits = indexer::mix_match(s.qdrant.as_ref(), &body.pos, &body.neg, body.limit)
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
    let path = PathBuf::from(&body.path);
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
    Ok(Json(json!({
        "path": body.path,
        "total": total,
        "indexed": report.indexed,
        "duplicates_skipped": report.duplicates_skipped,
        "errors": report.errors,
    })))
}

// ---- MCP over HTTP -------------------------------------------------------

/// POST /mcp — JSON-RPC 2.0 request in, response out. Same tools as stdio MCP.
/// Register from Claude CLI:  `claude mcp add --transport http memex-web http://localhost:<port>/mcp`
async fn mcp_post(State(s): State<WebState>, Json(body): Json<Value>) -> impl axum::response::IntoResponse {
    match mcp::handle_rpc_value(&s.mcp, body).await {
        Some(resp) => (StatusCode::OK, Json(resp)).into_response(),
        // Notification (no id) — JSON-RPC says no response body.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// GET /mcp — minimal SSE stream so the endpoint is reachable for SSE-style
/// clients; server→client streaming beyond a readiness ping is not used.
async fn mcp_sse() -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let stream = futures::stream::once(async {
        Ok(Event::default().event("ready").data("memex mcp sse ready"))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

use axum::response::IntoResponse;
