//! Stdio JSON-RPC MCP server — exposes Memex as a Model Context Protocol
//! server so any MCP-aware AI agent (Claude Code, Codex, Cursor, …) can call
//! Memex tools mid-conversation.
//!
//! Transport: newline-delimited JSON-RPC 2.0 over stdin/stdout, per the MCP
//! stdio transport profile. We hand-roll the protocol rather than depending
//! on a third-party crate so this stays self-contained and zero-surprise.
//!
//! Registered tools (semantic surface — Qdrant primitives are hidden):
//!   - find_similar_sessions
//!   - find_similar_error
//!   - predict_next_action
//!   - mix_similar_sessions
//!   - get_session_summary
//!   - get_session_turn
//!   - list_recent_sessions
//!   - analyze_corpus_topology
//!   - snapshot_export
//!   - **get_project_memory** — Cold Start Killer. Synthesizes a memory
//!     primer for the current working directory, distilling past decisions,
//!     pitfalls, and stack fingerprint from semantically-related past
//!     sessions. Drop the returned markdown into the agent's system message
//!     at session start and the agent stops starting cold.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex as AsyncMutex;

use crate::companion;
use crate::enrich;
use crate::indexer::{self, Embedder, LensWeights};
use crate::parser;
use crate::schema::COLLECTION_V3;
use crate::wrapped;

const SERVER_NAME: &str = "memex";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

/// Shared lazy state — same lazy-init pattern as commands::AppState.
/// Public so the `web` HTTP MCP endpoint can share it via `SharedMcpState`.
pub struct McpState {
    qdrant: AsyncMutex<Option<Arc<qdrant_client::Qdrant>>>,
    embedder: AsyncMutex<Option<Arc<Embedder>>>,
    /// Issue #16 Stage 2 — optional `WebMetrics` handle. `None` on the
    /// desktop stdio MCP path (desktop variant has no `/metrics` endpoint);
    /// `Some(...)` when wired by `web::serve` so the HTTP MCP write tool
    /// can call `state.mark_indexed(n)` and flip the `points_indexed_total`
    /// counter off zero.
    #[cfg(feature = "web")]
    metrics: Option<Arc<crate::web::WebMetrics>>,
}

impl McpState {
    fn new() -> Self {
        Self {
            qdrant: AsyncMutex::new(None),
            embedder: AsyncMutex::new(None),
            #[cfg(feature = "web")]
            metrics: None,
        }
    }

    /// Issue #16 Stage 2 — construct an MCP state pre-wired with a metrics
    /// handle. Used by `web::serve` so the HTTP MCP transport increments
    /// `points_indexed_total` when a write tool succeeds.
    #[cfg(feature = "web")]
    pub fn new_with_metrics(metrics: Arc<crate::web::WebMetrics>) -> Self {
        Self {
            qdrant: AsyncMutex::new(None),
            embedder: AsyncMutex::new(None),
            metrics: Some(metrics),
        }
    }

    /// Issue #16 Stage 2 — single entry point for the write-tool path to
    /// increment the indexed-points counter. No-op on the desktop variant
    /// (no `WebMetrics` linked), single atomic add on the web variant.
    #[cfg(feature = "web")]
    fn mark_indexed(&self, n: u64) {
        if let Some(m) = &self.metrics {
            m.mark_indexed(n);
        }
    }
    #[cfg(not(feature = "web"))]
    fn mark_indexed(&self, _n: u64) {
        // Desktop variant: no /metrics endpoint, no-op.
    }

    async fn qdrant(&self) -> Result<Arc<qdrant_client::Qdrant>> {
        let mut g = self.qdrant.lock().await;
        if let Some(c) = g.as_ref() {
            return Ok(c.clone());
        }
        let c = Arc::new(indexer::connect().await?);
        indexer::ensure_collection(&c).await?;
        *g = Some(c.clone());
        Ok(c)
    }

    async fn embedder(&self) -> Result<Arc<Embedder>> {
        let mut g = self.embedder.lock().await;
        if let Some(e) = g.as_ref() {
            return Ok(e.clone());
        }
        let e = Arc::new(
            tokio::task::spawn_blocking(Embedder::new)
                .await
                .map_err(|e| anyhow::anyhow!("embedder init task panicked: {e}"))??,
        );
        *g = Some(e.clone());
        Ok(e)
    }
}

/// Entry point — called from `memex mcp` CLI subcommand.
pub async fn run() -> Result<()> {
    let state = Arc::new(McpState::new());

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    eprintln!("[memex-mcp] serving stdio · pid={}", std::process::id());

    while let Some(line) = reader.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let req: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[memex-mcp] parse error: {e} · line: {}", &line[..line.len().min(160)]);
                continue;
            }
        };
        if req.jsonrpc != "2.0" {
            eprintln!("[memex-mcp] non-2.0 request ignored");
            continue;
        }

        let id = req.id.clone();
        // Notifications (no id) — handle silently.
        let is_notification = id.is_none();

        let result = dispatch(&state, &req.method, req.params).await;

        // Notifications get no response per JSON-RPC spec.
        if is_notification {
            continue;
        }

        let response = match result {
            Ok(value) => JsonRpcResponse {
                jsonrpc: "2.0",
                id: id.unwrap_or(Value::Null),
                result: Some(value),
                error: None,
            },
            Err(e) => JsonRpcResponse {
                jsonrpc: "2.0",
                id: id.unwrap_or(Value::Null),
                result: None,
                error: Some(JsonRpcError {
                    code: -32000,
                    message: format!("{e:#}"),
                    data: None,
                }),
            },
        };
        let mut bytes = serde_json::to_vec(&response)?;
        bytes.push(b'\n');
        stdout.write_all(&bytes).await?;
        stdout.flush().await?;
    }

    Ok(())
}

/// Shared, lazily-initialized MCP state, reusable across transports
/// (stdio in `run()` above, and the HTTP `/mcp` endpoint in `web.rs`).
pub type SharedMcpState = Arc<McpState>;

/// Create a fresh shared MCP state (Qdrant + embedder init on first use).
pub fn new_shared_state() -> SharedMcpState {
    Arc::new(McpState::new())
}

/// Issue #16 Stage 2 — create a shared MCP state pre-wired with a metrics
/// handle. Only meaningful on the web variant; used by `web::serve` so
/// the HTTP MCP transport increments `points_indexed_total` when a write
/// tool succeeds.
#[cfg(feature = "web")]
pub fn new_shared_state_with_metrics(metrics: Arc<crate::web::WebMetrics>) -> SharedMcpState {
    Arc::new(McpState::new_with_metrics(metrics))
}

/// Handle one JSON-RPC request value and return the response value, reusing the
/// exact same `dispatch` as the stdio transport so HTTP MCP exposes identical
/// tools. Returns `None` for notifications (no `id`), per the JSON-RPC spec.
pub async fn handle_rpc_value(state: &SharedMcpState, body: Value) -> Option<Value> {
    let req: JsonRpcRequest = match serde_json::from_value(body) {
        Ok(r) => r,
        Err(e) => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": { "code": -32700, "message": format!("parse error: {e}") }
            }))
        }
    };
    if req.jsonrpc != "2.0" {
        return Some(json!({
            "jsonrpc": "2.0",
            "id": req.id.unwrap_or(Value::Null),
            "error": { "code": -32600, "message": "invalid request: jsonrpc must be \"2.0\"" }
        }));
    }
    let id = req.id.clone();
    let is_notification = id.is_none();
    let result = dispatch(state, &req.method, req.params).await;
    if is_notification {
        return None;
    }
    let response = match result {
        Ok(value) => JsonRpcResponse {
            jsonrpc: "2.0",
            id: id.unwrap_or(Value::Null),
            result: Some(value),
            error: None,
        },
        Err(e) => JsonRpcResponse {
            jsonrpc: "2.0",
            id: id.unwrap_or(Value::Null),
            result: None,
            error: Some(JsonRpcError {
                code: -32000,
                message: format!("{e:#}"),
                data: None,
            }),
        },
    };
    Some(serde_json::to_value(response).unwrap_or(Value::Null))
}

async fn dispatch(state: &Arc<McpState>, method: &str, params: Value) -> Result<Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION }
        })),
        // notifications/initialized has no result; we already short-circuited.
        "notifications/initialized" => Ok(Value::Null),
        "tools/list" => Ok(tools_catalog()),
        "tools/call" => tool_call(state, params).await,
        "ping" => Ok(json!({})),
        _ => Err(anyhow::anyhow!("method not found: {method}")),
    }
}

// Issue #16 Stage 2 — SHIPPED.
//
// The `refresh_session_enrich` write tool below now calls
// `state.mark_indexed(1)` on success. On the HTTP MCP transport (Docker
// server variant) this finally flips `memex_points_indexed_total` off
// zero. On the desktop stdio MCP path `state.mark_indexed` is a no-op
// because the desktop variant has no `/metrics` endpoint anyway, and
// metric-set uniformity across transports is what Prometheus best
// practice asks for (always-zero counters communicate that the
// dimension exists, just isn't active in this deployment —
// https://prometheus.io/docs/practices/instrumentation/ and the
// Google SRE Book Ch. 6).
//
// Future MCP write tools follow the same pattern: do the work, then
// call `state.mark_indexed(n)` where `n` is the number of points that
// changed. The helper is `#[cfg]`-gated so the desktop build stays
// metrics-free.

fn tools_catalog() -> Value {
    json!({
        "tools": [
            {
                "name": "find_similar_sessions",
                "description": "Find past Claude Code sessions semantically similar to a free-text query. Searches Memex's local Qdrant index across five named vectors (content, tool, path, error, code) and returns ranked sessions with per-vector contribution scores.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Free-text query." },
                        "limit": { "type": "integer", "description": "Max results.", "default": 10 },
                        "weights": {
                            "type": "object",
                            "description": "Per-vector weights (default 1.0 each). Slide a weight to 0 to drop that lens.",
                            "properties": {
                                "content": { "type": "number" },
                                "tool":    { "type": "number" },
                                "path":    { "type": "number" },
                                "error":   { "type": "number" },
                                "code":    { "type": "number" }
                            }
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "find_similar_error",
                "description": "Find past sessions where a similar error was encountered. Searches Memex's dedicated `error` named vector with a payload filter requiring has_errors=true. Returns sessions that *also produced an error* near the input — typically the past sessions that contain a resolution.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "error_text": { "type": "string", "description": "Error message or stack trace." },
                        "limit": { "type": "integer", "default": 5 }
                    },
                    "required": ["error_text"]
                }
            },
            {
                "name": "predict_next_action",
                "description": "Given a session, predict the agent/user's next likely tool calls by mining how similar past sessions proceeded from a comparable conversational position. Returns a ranked list of (tool_name, example_input, source_session, turn_index) with frequency × similarity confidence.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session_id": { "type": "string", "description": "Active session ID." },
                        "last_n_turns": { "type": "integer", "default": 3, "description": "How many recent turns to use as context." },
                        "horizon": { "type": "integer", "default": 3, "description": "How many turns ahead to walk in each neighbor." },
                        "neighbors": { "type": "integer", "default": 8, "description": "How many similar past sessions to consult." }
                    },
                    "required": ["session_id"]
                }
            },
            {
                "name": "mix_similar_sessions",
                "description": "Recommendation via Qdrant's Discovery API. Provide one or more positive session IDs (anchors) and zero or more negatives (anti-context); Qdrant returns sessions semantically near the positives and far from the negatives.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "positive": { "type": "array", "items": { "type": "string" }, "description": "Positive session IDs." },
                        "negative": { "type": "array", "items": { "type": "string" }, "default": [] },
                        "limit":    { "type": "integer", "default": 10 }
                    },
                    "required": ["positive"]
                }
            },
            {
                "name": "get_session_summary",
                "description": "Fetch a session's high-level metadata (project, branch, ai_title, start/end, turn counts, has_errors) from the Qdrant payload. Use this to understand a session at a glance before calling get_session_turn for detail.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session_id": { "type": "string" }
                    },
                    "required": ["session_id"]
                }
            },
            {
                "name": "get_session_turn",
                "description": "Retrieve a single turn of a session by index — full text + tool calls + tool results, with sidechain flag. Source jsonl is re-parsed on demand so Qdrant payloads stay small.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session_id": { "type": "string" },
                        "turn_index": { "type": "integer", "description": "0-based turn index." }
                    },
                    "required": ["session_id", "turn_index"]
                }
            },
            {
                "name": "list_recent_sessions",
                "description": "List sessions in ~/.claude/projects sorted most-recent first, with project name, git branch, turn counts, has_errors. Independent of Qdrant — works even before the index is fully warmed up.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "default": 30 }
                    }
                }
            },
            {
                "name": "analyze_corpus_topology",
                "description": "Return the structure of the user's entire session corpus: MST of session content vectors, per-project auto-labels (top tools, top paths, theme), cross-project bridge counts, and gap insights flagging project pairs that are semantically close but never bridged.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "sample":    { "type": "integer", "default": 80 },
                        "per_point": { "type": "integer", "default": 6 }
                    }
                }
            },
            {
                "name": "snapshot_export",
                "description": "Export the entire Memex Qdrant collection as a single .snapshot file at the given path. Portable across machines.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Destination file path." }
                    },
                    "required": ["path"]
                }
            },
            {
                "name": "get_project_memory",
                "description": "**Memex Cold Start Killer.** Given a working directory, returns a ready-to-inject memory primer for the agent. Mines past sessions in that codebase (or semantically similar projects) and surfaces: original intents, committed decisions (\"I'll use NextAuth\", \"Stack: Next.js + Drizzle\"), known pitfalls (errors the user already hit), and the stack fingerprint (top tools, file extensions, bash binaries). The response includes `markdown` — drop it verbatim into the next session's system prompt. Zero LLM in the loop; the primer is deterministically distilled from the local Qdrant index.\n\nCALL THIS AT TURN 0 of any Claude Code / Codex session. It is the difference between an agent that re-asks the user the same questions every session and one that resumes where past-you left off.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "cwd": {
                            "type": "string",
                            "description": "Absolute working directory for the new session. If omitted, uses the Memex process' own cwd (usually only useful in CLI smoke tests)."
                        },
                        "limit": {
                            "type": "integer",
                            "default": 8,
                            "description": "Max past sessions to mine. Caps the size of the markdown primer."
                        }
                    }
                }
            },
            {
                "name": "generate_wrapped_report",
                "description": "**Memex Wrapped — engineering 'Spotify Wrapped'.** Returns a one-page corpus digest for the user's last `window_days` days (default 30, set to 0 for all-time). Aggregates the entire local Qdrant index into: top tools, top bash binaries, top file extensions, intent / arc / outcome distribution, repeated decisions (the 'I keep re-deciding the same thing' signal), debugging fingerprint, and cross-agent split when both Claude Code + Codex sessions are present. Zero LLM in the loop; deterministic aggregation. The response includes `markdown` formatted for screenshot sharing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "window_days": {
                            "type": "integer",
                            "default": 30,
                            "description": "Time window in days. 0 = all-time."
                        },
                        "limit": {
                            "type": "integer",
                            "default": 32,
                            "description": "Max sessions to deep-mine for repeated-decision detection."
                        }
                    }
                }
            },
            {
                "name": "refresh_session_enrich",
                "description": "**[Write tool · Issue #16 Stage 2]** Re-runs the deterministic enrichment pipeline (intent · entities · outcome · arc · topic) for one indexed session and writes the result back to its Qdrant payload via SetPayload. Useful after the heuristics in `crate::enrich` are improved — call on legacy sessions to upgrade their labels in place, without re-embedding. This is the FIRST write tool in the MCP surface; on the HTTP MCP transport (Docker server variant) it flips `memex_points_indexed_total` off zero. Returns the new intent / outcome so the caller can see what changed.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "session_id": {
                            "type": "string",
                            "description": "Session id (UUID-ish string) that already exists in the v3 collection. Use `list_recent_sessions` or `find_similar_sessions` to discover ids."
                        }
                    },
                    "required": ["session_id"]
                }
            }
        ]
    })
}

async fn tool_call(state: &Arc<McpState>, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("tools/call missing 'name'"))?;
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);

    let result_json: Value = match name {
        "find_similar_sessions" => {
            let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);
            let weights: LensWeights = serde_json::from_value(
                args.get("weights").cloned().unwrap_or(Value::Null),
            )
            .unwrap_or_default();
            let qdrant = state.qdrant().await?;
            let embedder = state.embedder().await?;
            let hits = indexer::lens_search(&qdrant, &embedder, &query, &weights, limit, 60).await?;
            serde_json::to_value(hits)?
        }
        "find_similar_error" => {
            let text = args.get("error_text").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(5);
            let qdrant = state.qdrant().await?;
            let embedder = state.embedder().await?;
            let hits = indexer::recall(&qdrant, &embedder, &text, limit).await?;
            serde_json::to_value(hits)?
        }
        "predict_next_action" => {
            let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let last_n = args.get("last_n_turns").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let horizon = args.get("horizon").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
            let neighbors = args.get("neighbors").and_then(|v| v.as_u64()).unwrap_or(8);
            let qdrant = state.qdrant().await?;
            let embedder = state.embedder().await?;
            let ctx = indexer::predict_next_actions(&qdrant, &embedder, &session_id, last_n, horizon, neighbors).await?;
            serde_json::to_value(ctx)?
        }
        "mix_similar_sessions" => {
            let positive: Vec<String> = args
                .get("positive")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let negative: Vec<String> = args
                .get("negative")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10);
            let qdrant = state.qdrant().await?;
            let hits = indexer::mix_match(&qdrant, &positive, &negative, limit).await?;
            serde_json::to_value(hits)?
        }
        "get_session_summary" => {
            let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
            let qdrant = state.qdrant().await?;
            let payload = indexer::get_session_payload(&qdrant, session_id).await?;
            match payload {
                Some(p) => {
                    let mut out = serde_json::Map::new();
                    for (k, v) in p {
                        out.insert(k, qdrant_value_to_json(v));
                    }
                    Value::Object(out)
                }
                None => Value::Null,
            }
        }
        "get_session_turn" => {
            let session_id = args.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
            let turn_index = args.get("turn_index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let qdrant = state.qdrant().await?;
            let payload = indexer::get_session_payload(&qdrant, session_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("session not in index"))?;
            let source = payload
                .get("source_path")
                .and_then(|v| v.kind.as_ref())
                .and_then(|k| match k {
                    qdrant_client::qdrant::value::Kind::StringValue(s) => Some(s.clone()),
                    _ => None,
                })
                .ok_or_else(|| anyhow::anyhow!("payload missing source_path"))?;
            let session = parser::parse_session(std::path::Path::new(&source))?;
            let turn = session
                .turns
                .get(turn_index)
                .ok_or_else(|| anyhow::anyhow!("turn_index {} out of range (max {})", turn_index, session.turns.len().saturating_sub(1)))?;
            serde_json::to_value(turn)?
        }
        "list_recent_sessions" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
            // Unify modern projects/ + legacy transcripts/ — same behaviour as
            // the Tauri command `list_sessions` so any MCP client sees the
            // same corpus the desktop UI does (pre-v2.1.114 transcripts
            // included).
            let mut sessions = parser::scan_dir(&default_projects_root())?;
            if let Ok(legacy) = parser::scan_transcripts_dir(&default_transcripts_root()) {
                sessions.extend(legacy);
            }
            sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));
            let summaries: Vec<crate::summary::SessionSummary> = sessions
                .into_iter()
                .take(limit)
                .map(crate::summary::SessionSummary::from)
                .collect();
            serde_json::to_value(summaries)?
        }
        "analyze_corpus_topology" => {
            let sample = args.get("sample").and_then(|v| v.as_u64()).unwrap_or(80) as u32;
            let per_point = args.get("per_point").and_then(|v| v.as_u64()).unwrap_or(6) as u32;
            let qdrant = state.qdrant().await?;
            let topo = indexer::topology(&qdrant, sample, per_point, Some(default_projects_root())).await?;
            serde_json::to_value(topo)?
        }
        "snapshot_export" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("path required"))?;
            let name = indexer::snapshot_export(std::path::Path::new(path)).await?;
            json!({ "snapshot_name": name, "path": path })
        }
        "get_project_memory" => {
            // Resolve cwd from args; default to the Memex process cwd if the
            // agent forgot to pass one (Claude Code typically supplies it via
            // its tool_use input, but we don't want to hard-fail on omission).
            let cwd_arg = args
                .get("cwd")
                .and_then(|v| v.as_str())
                .map(std::path::PathBuf::from);
            let cwd = companion::resolve_cwd_arg(cwd_arg.as_deref())?;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(8) as usize;
            let qdrant = state.qdrant().await?;
            // PR7-A — lazy embedder. Local-project hits use the
            // project_name keyword scroll only (no embedder), so the
            // common "resume in this directory" case never pays the
            // ~130MB BGE-small ONNX init. Only when no local match
            // exists do we lazy-load the embedder for the cross-project
            // semantic-neighbor pass. PR #8 follow-up #1 — centralized
            // in `companion::compose_memory_primer_lazy_load` so the
            // peek-then-load branch isn't hand-rolled at every caller.
            let primer = companion::compose_memory_primer_lazy_load(
                &qdrant,
                &cwd,
                limit,
                || async { state.embedder().await },
            )
            .await?;
            serde_json::to_value(primer)?
        }
        "generate_wrapped_report" => {
            let window_days =
                args.get("window_days").and_then(|v| v.as_u64()).unwrap_or(30) as u32;
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(32) as usize;
            let qdrant = state.qdrant().await?;
            // Wrapped is payload-only — no embedder needed. (Codex P2-a.)
            let report =
                wrapped::compose_wrapped(&qdrant, window_days, limit).await?;
            serde_json::to_value(report)?
        }
        // ─────────────────────────────────────────────────────────────────
        // Issue #16 Stage 2 — first MCP write tool. Re-runs enrich() on one
        // indexed session and writes the deterministic labels back via
        // SetPayload. No re-embedding (payload-only update); idempotent
        // (same input → same output). On the HTTP MCP transport this is
        // what finally flips `memex_points_indexed_total` off zero.
        // ─────────────────────────────────────────────────────────────────
        "refresh_session_enrich" => {
            let session_id = args
                .get("session_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("refresh_session_enrich: missing 'session_id'"))?;
            let qdrant = state.qdrant().await?;

            // 1. Confirm the session is already in the v3 collection — and
            //    pull its `source_path` so we know where to re-parse from.
            let payload = indexer::get_session_payload(&qdrant, session_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("session not in index: {session_id}"))?;
            let source_path = indexer::payload_str(&payload, "source_path")
                .ok_or_else(|| anyhow::anyhow!("payload missing source_path for {session_id}"))?;

            // 2. Validate the path against the sandbox roots BEFORE we read
            //    it (Gemini #22 review — HIGH security). `source_path` came
            //    out of Qdrant payload, which is attacker-influenceable in
            //    principle (anyone who can write the payload could put
            //    `/etc/passwd` here). `parser::parse_session` does not run
            //    its own traversal guard, so we run the same `sec`
            //    validation every other ingress path uses (web.rs · indexer).
            //
            //    Then route to the matching parser by `source_agent`: Claude
            //    Code sessions use the default JSONL envelope, Codex
            //    sessions use `codex_parser` (different envelope shape).
            //    Default to Claude-Code parsing when the payload is missing
            //    the field (legacy v2 points lifted to v3 may have no
            //    source_agent).
            let session_path = PathBuf::from(&source_path);
            let validated_path = crate::sec::validate_session_path(&session_path)
                .map_err(|e| anyhow::anyhow!("path validation failed for {session_id}: {e:#}"))?;
            let source_agent = indexer::payload_str(&payload, "source_agent")
                .unwrap_or_else(|| "claude_code".to_string());
            let session = if source_agent == "codex" {
                crate::codex_parser::parse_codex_session(&validated_path)
            } else {
                parser::parse_session(&validated_path)
            }
            .map_err(|e| anyhow::anyhow!("re-parse failed for {session_id}: {e:#}"))?;

            // 3. Re-run the deterministic enrichment pipeline. Same code
            //    path the indexer uses on initial upsert.
            let out = enrich::enrich(&session, &session.turns);

            // 4. Build a payload-only update that touches just the five
            //    enrich-stage fields. The dense / sparse / multivector
            //    vectors are untouched — SetPayload doesn't re-embed.
            let new_payload_json = json!({
                "intent":   out.intent,
                "entities": out.entities,
                "outcome":  out.outcome,
                "arc":      out.arc,
                "topic":    out.topic,
            });
            let new_payload = qdrant_client::Payload::try_from(new_payload_json)?;

            // 5. SetPayload on exactly this one point. We construct the
            //    PointId from `indexer::point_id` to match the same UUID v5
            //    scheme the indexer uses on insert.
            use qdrant_client::qdrant::{
                point_id::PointIdOptions, points_selector::PointsSelectorOneOf,
                PointId, PointsIdsList, SetPayloadPointsBuilder,
            };
            let pid = PointId {
                point_id_options: Some(PointIdOptions::Uuid(indexer::point_id(session_id))),
            };
            let req = SetPayloadPointsBuilder::new(COLLECTION_V3, new_payload)
                .points_selector(PointsSelectorOneOf::Points(PointsIdsList { ids: vec![pid] }))
                .wait(true);
            qdrant.set_payload(req).await?;

            // 6. Issue #16 Stage 2 — flip the counter. On the desktop stdio
            //    MCP path this is a no-op (no /metrics endpoint exists);
            //    on the HTTP MCP transport it adds 1 to the shared
            //    `memex_points_indexed_total` family.
            state.mark_indexed(1);

            json!({
                "session_id": session_id,
                "updated":    1,
                "intent":     out.intent,
                "outcome":    out.outcome,
                "arc":        out.arc,
                "topic":      out.topic,
                "entities":   out.entities,
            })
        }
        other => return Err(anyhow::anyhow!("unknown tool: {other}")),
    };

    // MCP CallToolResult wraps payload as a content array of text blocks.
    let text = serde_json::to_string_pretty(&result_json)?;
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "isError": false
    }))
}

fn qdrant_value_to_json(v: qdrant_client::qdrant::Value) -> Value {
    use qdrant_client::qdrant::value::Kind;
    match v.kind {
        Some(Kind::NullValue(_)) | None => Value::Null,
        Some(Kind::BoolValue(b)) => Value::Bool(b),
        Some(Kind::IntegerValue(i)) => Value::Number(i.into()),
        Some(Kind::DoubleValue(d)) => serde_json::Number::from_f64(d)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Some(Kind::StringValue(s)) => Value::String(s),
        Some(Kind::ListValue(l)) => Value::Array(l.values.into_iter().map(qdrant_value_to_json).collect()),
        Some(Kind::StructValue(s)) => {
            let mut m = serde_json::Map::new();
            for (k, vv) in s.fields {
                m.insert(k, qdrant_value_to_json(vv));
            }
            Value::Object(m)
        }
    }
}

fn default_projects_root() -> PathBuf {
    // WIN-01: use `dirs::home_dir()` rather than `env::var("HOME")` — Windows
    // has no HOME (it uses %USERPROFILE%), so the env-var read returned an
    // empty corpus there. `dirs` resolves the platform-canonical home.
    if let Some(home) = dirs::home_dir() {
        home.join(".claude").join("projects")
    } else {
        PathBuf::from(".claude/projects")
    }
}

fn default_transcripts_root() -> PathBuf {
    // WIN-01: see default_projects_root() above.
    if let Some(home) = dirs::home_dir() {
        home.join(".claude").join("transcripts")
    } else {
        PathBuf::from(".claude/transcripts")
    }
}
