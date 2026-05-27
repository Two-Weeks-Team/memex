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
use crate::indexer::{self, Embedder, LensWeights};
use crate::parser;
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
}

impl McpState {
    fn new() -> Self {
        Self {
            qdrant: AsyncMutex::new(None),
            embedder: AsyncMutex::new(None),
        }
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
            // PR7-A — lazy embedder. First compose a LOCAL-ONLY primer (no
            // embedder): the common case is resuming a directory that already
            // has indexed sessions, served entirely from the project_name
            // keyword scroll — so we never pay the ~130MB BGE-small load and
            // the primer works offline / cold-start. Only when no local
            // project matched do we lazily init the embedder and recompose
            // with the cross-project semantic-neighbor pass.
            let primer = {
                let local =
                    companion::compose_memory_primer_lazy(&qdrant, None, &cwd, limit).await?;
                if local.matched_local_project {
                    local
                } else {
                    let embedder = state.embedder().await?;
                    companion::compose_memory_primer_lazy(&qdrant, Some(&embedder), &cwd, limit)
                        .await?
                }
            };
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
