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
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use crate::{crud, indexer, lens, mcp, parser, retrieval, schema, sec};

#[derive(Clone)]
struct WebState {
    qdrant: Arc<qdrant_client::Qdrant>,
    embedder: Arc<indexer::Embedder>,
    mcp: mcp::SharedMcpState,
    ui_dir: PathBuf,
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

    let state = WebState {
        qdrant,
        embedder,
        mcp: mcp::new_shared_state(),
        ui_dir: ui_dir.clone(),
    };

    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/search", get(search))
        .route("/api/lens", post(lens))
        .route("/api/recall", get(recall))
        .route("/api/topology", get(topology))
        .route("/api/mix", post(mix))
        .route("/api/index", post(index_path))
        // Generic Tauri-command bridge: the browser UI's __TAURI__ shim posts
        // here so the existing frontend works unchanged over HTTP.
        .route("/api/invoke/{cmd}", post(invoke_handler))
        .route("/mcp", post(mcp_post).get(mcp_sse));

    let app = Router::new()
        .merge(api)
        // Serve index.html with the __TAURI__ fetch shim injected (web only).
        .route("/", get(index_html))
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
                .ok_or((StatusCode::BAD_REQUEST, "session payload missing source_path".to_string()))?;
            let validated = sec::validate_session_path(std::path::Path::new(&source)).map_err(err500)?;
            ok_json(parser::parse_session(&validated).map_err(err500)?)
        }
        "list_sessions" => {
            let root = a_str(&args, "path").map(PathBuf::from).unwrap_or_else(web_scan_root);
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
            let root = a_str(&args, "path").map(PathBuf::from).unwrap_or_else(web_scan_root);
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
        "snapshot_export_default" => {
            let dir = std::env::var("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
            let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
            let path = dir.join(format!("memex-snapshot-{ts}.snapshot"));
            let name = indexer::snapshot_export(&path).await.map_err(err500)?;
            ok_json(json!({ "name": name, "path": path.display().to_string() }))
        }
        other => Err((StatusCode::NOT_FOUND, format!("unknown command: {other}"))),
    }
}

/// Serve index.html with the __TAURI__ fetch shim injected so the existing
/// browser frontend's `invoke()`/`event.listen()` calls work over HTTP. Only
/// the web server hits this path — the Tauri desktop app loads index.html
/// directly and is unaffected.
async fn index_html(State(s): State<WebState>) -> Response {
    let path = s.ui_dir.join("index.html");
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
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
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
