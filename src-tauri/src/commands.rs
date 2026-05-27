//! Tauri command surface — what the frontend can `invoke()`.
//!
//! Each command takes `State<AppState>` (a long-lived holder of the Qdrant
//! client + Embedder) and returns `Result<T, String>` so errors can cross the
//! IPC boundary.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::Context;
use once_cell::sync::Lazy;
use qdrant_client::Qdrant;
use tauri::State;
use tokio::sync::Mutex as AsyncMutex;

use crate::indexer::{
    self, Embedder, LensWeights, SearchHit, Topology, COLLECTION,
};
use crate::parser;

/// AppState holds the heavyweight resources (Qdrant client + fastembed model)
/// behind lazy slots. `.manage()` is called eagerly in `lib.rs::run()` so the
/// state container is always present; the actual init happens on first command
/// invocation. If init fails (e.g. Qdrant is down at launch), the slot stays
/// empty and the *next* call retries — so the app self-heals as soon as the
/// user starts Qdrant in another terminal.
pub struct AppState {
    qdrant: AsyncMutex<Option<Arc<Qdrant>>>,
    embedder: AsyncMutex<Option<Arc<Embedder>>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            qdrant: AsyncMutex::new(None),
            embedder: AsyncMutex::new(None),
        }
    }

    pub async fn qdrant(&self) -> anyhow::Result<Arc<Qdrant>> {
        let mut guard = self.qdrant.lock().await;
        if let Some(c) = guard.as_ref() {
            return Ok(c.clone());
        }
        let client = Arc::new(
            indexer::connect()
                .await
                .context("could not connect to Qdrant — is it running on localhost:6334?")?,
        );
        indexer::ensure_collection(&client)
            .await
            .context("connected to Qdrant but failed to ensure the collection schema")?;
        // P3 KG-03 — v3 collection lives alongside v2. Idempotent.
        crate::crud::ensure_collection_v3(&client)
            .await
            .context("failed to ensure v3 collection schema")?;

        // FUNCTIONAL FIX (Codex review on PR #3, commands.rs:64): on upgrade
        // from a Memex install that only ever wrote v2, the v3 collection is
        // empty at startup, so every search/topology/recall returns nothing
        // until the user manually triggers `scan --index`. Auto-trigger the
        // v2→v3 carry-forward as a background task so the UI stays
        // responsive (the migration is paginated, but on a large v2 it may
        // take minutes). The task is best-effort — failures are logged but
        // don't block the app from running.
        let client_for_bg = client.clone();
        tokio::spawn(async move {
            match crate::crud::migrate_v2_to_v3_if_needed(&client_for_bg).await {
                Ok(Some(report)) => {
                    eprintln!(
                        "[memex] v2→v3 migration done: {} new points (v2 had {}, v3 was empty), {} skipped, took {}ms",
                        report.migrated, report.v2_count, report.v2_count.saturating_sub(report.migrated), report.elapsed_ms,
                    );
                }
                Ok(None) => {
                    // Either v2 is empty or v3 already has data — no-op.
                }
                Err(e) => {
                    // LOG FIX (Gemini PR #11 review, commands.rs:87): Err
                    // means migration *failed* mid-flight (connectivity,
                    // auth, server panic) — NOT a routine skip. The
                    // Ok(None) arm above is the actual no-op case. Word
                    // accordingly so a postmortem can grep for "failed".
                    eprintln!("[memex] v2→v3 migration failed: {e:#}");
                }
            }
        });

        *guard = Some(client.clone());
        Ok(client)
    }

    pub async fn embedder(&self) -> anyhow::Result<Arc<Embedder>> {
        let mut guard = self.embedder.lock().await;
        if let Some(e) = guard.as_ref() {
            return Ok(e.clone());
        }
        // Embedder::new is synchronous (ONNX model load); run on the blocking
        // pool so we don't park the tokio worker for the ~130 MB first-time
        // download.
        let embedder = tokio::task::spawn_blocking(Embedder::new)
            .await
            .context("embedder init task panicked")?
            .context("failed to load BGE-small-en-v1.5 — check ~/.fastembed_cache/")?;
        let arc = Arc::new(embedder);
        *guard = Some(arc.clone());
        Ok(arc)
    }
}

pub type AppStateArc = Arc<AppState>;

fn stringify<E: std::fmt::Display>(e: E) -> String {
    format!("{e:#}")
}

#[tauri::command]
pub async fn lens_search(
    state: State<'_, AppStateArc>,
    query: String,
    weights: Option<LensWeights>,
    limit: Option<u64>,
) -> Result<Vec<SearchHit>, String> {
    let weights = weights.unwrap_or_default();
    let limit = limit.unwrap_or(20);
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let embedder = state.embedder().await.map_err(stringify)?;
    indexer::lens_search(&qdrant, &embedder, &query, &weights, limit, 60)
        .await
        .map_err(stringify)
}

#[tauri::command]
pub async fn mix_match(
    state: State<'_, AppStateArc>,
    positive: Vec<String>,
    negative: Vec<String>,
    limit: Option<u64>,
) -> Result<Vec<SearchHit>, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    indexer::mix_match(&qdrant, &positive, &negative, limit.unwrap_or(20))
        .await
        .map_err(stringify)
}

#[tauri::command]
pub async fn topology(
    state: State<'_, AppStateArc>,
    sample: Option<u32>,
    per_point: Option<u32>,
    path: Option<PathBuf>,
) -> Result<Topology, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    // Default to ~/.claude/projects so the response carries project_insights
    // + gap_insights (auto-labels + gap analysis).
    let projects_root = Some(path.unwrap_or_else(default_projects_root));
    indexer::topology(
        &qdrant,
        sample.unwrap_or(80),
        per_point.unwrap_or(5),
        projects_root,
    )
    .await
    .map_err(stringify)
}

#[tauri::command]
pub async fn recall(
    state: State<'_, AppStateArc>,
    error_text: String,
    limit: Option<u64>,
) -> Result<Vec<SearchHit>, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let embedder = state.embedder().await.map_err(stringify)?;
    indexer::recall(&qdrant, &embedder, &error_text, limit.unwrap_or(5))
        .await
        .map_err(stringify)
}

#[tauri::command]
pub async fn get_session(
    state: State<'_, AppStateArc>,
    session_id: String,
) -> Result<Option<serde_json::Value>, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let payload = indexer::get_session_payload(&qdrant, &session_id)
        .await
        .map_err(stringify)?;
    match payload {
        None => Ok(None),
        Some(p) => {
            let mut out = serde_json::Map::new();
            for (k, v) in p {
                out.insert(k, qdrant_value_to_json(v));
            }
            Ok(Some(serde_json::Value::Object(out)))
        }
    }
}

#[tauri::command]
pub async fn get_session_turns(
    state: State<'_, AppStateArc>,
    session_id: String,
) -> Result<serde_json::Value, String> {
    // Pull the payload to find the original source jsonl path, then re-parse
    // it so the replay can stream turn-by-turn without bloating Qdrant payloads.
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let payload = indexer::get_session_payload(&qdrant, &session_id)
        .await
        .map_err(stringify)?;
    let Some(payload) = payload else {
        return Err(format!("session {session_id} not in index"));
    };
    let source = payload
        .get("source_path")
        .and_then(|v| v.kind.as_ref())
        .and_then(|k| match k {
            qdrant_client::qdrant::value::Kind::StringValue(s) => Some(s.clone()),
            _ => None,
        })
        .ok_or_else(|| "session payload missing source_path".to_string())?;
    let validated = crate::sec::validate_session_path(std::path::Path::new(&source))
        .map_err(stringify)?;
    let session = parser::parse_session(&validated).map_err(stringify)?;
    serde_json::to_value(&session).map_err(stringify)
}

fn qdrant_value_to_json(v: qdrant_client::qdrant::Value) -> serde_json::Value {
    use qdrant_client::qdrant::value::Kind;
    use serde_json::Value as J;
    match v.kind {
        Some(Kind::NullValue(_)) | None => J::Null,
        Some(Kind::BoolValue(b)) => J::Bool(b),
        Some(Kind::IntegerValue(i)) => J::Number(i.into()),
        Some(Kind::DoubleValue(d)) => serde_json::Number::from_f64(d)
            .map(J::Number)
            .unwrap_or(J::Null),
        Some(Kind::StringValue(s)) => J::String(s),
        Some(Kind::ListValue(l)) => {
            J::Array(l.values.into_iter().map(qdrant_value_to_json).collect())
        }
        Some(Kind::StructValue(s)) => {
            let mut m = serde_json::Map::new();
            for (k, vv) in s.fields {
                m.insert(k, qdrant_value_to_json(vv));
            }
            J::Object(m)
        }
    }
}

#[tauri::command]
pub async fn predict_next_actions(
    state: State<'_, AppStateArc>,
    session_id: String,
    last_n_turns: Option<usize>,
    horizon: Option<usize>,
    neighbors: Option<u64>,
) -> Result<indexer::PredictionContext, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let embedder = state.embedder().await.map_err(stringify)?;
    indexer::predict_next_actions(
        &qdrant,
        &embedder,
        &session_id,
        last_n_turns.unwrap_or(3),
        horizon.unwrap_or(3),
        neighbors.unwrap_or(8),
    )
    .await
    .map_err(stringify)
}

/// **Wrapped (GUI surface).** Compose a corpus-wide digest covering the
/// last `window_days` days (0 = all-time). LLM-free, embedder-free —
/// pure aggregation over Qdrant payload + JSONL re-parse for decisions.
#[tauri::command]
pub async fn compose_wrapped(
    state: State<'_, AppStateArc>,
    window_days: Option<u32>,
    limit: Option<usize>,
) -> Result<crate::wrapped::WrappedReport, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    crate::wrapped::compose_wrapped(
        &qdrant,
        window_days.unwrap_or(30),
        limit.unwrap_or(32),
    )
    .await
    .map_err(stringify)
}

/// **Cold Start Killer (GUI surface).** Compose a memory primer for the
/// given cwd. The frontend Companion panel calls this on demand (or in
/// response to a watcher event that flagged a freshly-opened session).
#[tauri::command]
pub async fn compose_memory_primer(
    state: State<'_, AppStateArc>,
    cwd: Option<String>,
    limit: Option<usize>,
) -> Result<crate::companion::MemoryPrimer, String> {
    let cwd_path = match cwd.as_deref() {
        Some(s) if !s.is_empty() => Some(std::path::PathBuf::from(s)),
        _ => None,
    };
    let resolved = crate::companion::resolve_cwd_arg(cwd_path.as_deref()).map_err(stringify)?;
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let embedder = state.embedder().await.map_err(stringify)?;
    crate::companion::compose_memory_primer(&qdrant, &embedder, &resolved, limit.unwrap_or(8))
        .await
        .map_err(stringify)
}

#[tauri::command]
pub async fn snapshot_export(path: PathBuf) -> Result<String, String> {
    let sb = crate::snapshot::SnapshotSandbox::from_env().map_err(stringify)?;
    let canonical = sb
        .validate_path(&path, crate::snapshot::SnapshotOp::Export)
        .map_err(stringify)?;
    let name = indexer::snapshot_export(&canonical).await.map_err(stringify)?;
    // ATOMICITY FIX (CodeRabbit PR #2 review, commands.rs:254): if signing
    // fails we'd leave an unsigned snapshot in the sandbox that subsequent
    // exports refuse to overwrite (sandbox `Export` op rejects existing
    // files) AND that imports happily accept as "legacy unsigned" — both
    // wrong outcomes. Delete the unsigned bytes on sign failure so the user
    // can re-run export cleanly, and surface the sign error to them.
    if let Err(sign_err) = crate::snapshot::SignedEnvelope::sign(&canonical) {
        let cleanup = std::fs::remove_file(&canonical);
        return Err(format!(
            "snapshot signing failed: {sign_err:#}; rollback {}",
            match cleanup {
                Ok(()) => "ok".to_string(),
                Err(e) => format!("ALSO FAILED: {e}"),
            }
        ));
    }
    Ok(name)
}

/// Convenience wrapper called from the dashboard's "data archaeology" card —
/// drops a snapshot into the user's home dir with an ISO-timestamped name
/// so the user doesn't have to type a path. Returns "<name> → <abs_path>"
/// so the UI can echo where the file landed.
#[tauri::command]
pub async fn snapshot_export_default() -> Result<String, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H-%M-%S").to_string();
    let path = PathBuf::from(&home).join(format!("memex-snapshot-{ts}.snapshot"));
    let name = indexer::snapshot_export(&path).await.map_err(stringify)?;
    Ok(format!("{name} → {}", path.display()))
}

/// Imports a snapshot from the sandboxed snapshot directory after envelope
/// verification. Returns an empty string on clean import (envelope `Ok`) and a
/// human-readable warning on the three non-fatal outcomes (legacy / schema
/// drift / Qdrant minor drift). Tampered or major-version-incompatible
/// snapshots are rejected with `Err`. Audit MED-1.
#[tauri::command]
pub async fn snapshot_import(path: PathBuf) -> Result<String, String> {
    let sb = crate::snapshot::SnapshotSandbox::from_env().map_err(stringify)?;
    let canonical = sb
        .validate_path(&path, crate::snapshot::SnapshotOp::Import)
        .map_err(stringify)?;
    let warning = match crate::snapshot::SignedEnvelope::verify(&canonical).map_err(stringify)? {
        crate::snapshot::VerifyOutcome::Ok => String::new(),
        crate::snapshot::VerifyOutcome::LegacyNoSignature => {
            let msg = "snapshot has no signature — legacy import allowed".to_string();
            eprintln!("[memex] {msg}");
            msg
        }
        crate::snapshot::VerifyOutcome::WarnSchemaMismatch { expected, found } => {
            let msg = format!("snapshot schema {found} differs from current {expected} — proceeding");
            eprintln!("[memex] {msg}");
            msg
        }
        crate::snapshot::VerifyOutcome::WarnQdrantMinor { expected, found } => {
            let msg = format!("snapshot qdrant {found} differs from current {expected} — proceeding");
            eprintln!("[memex] {msg}");
            msg
        }
    };
    indexer::snapshot_import(&canonical).await.map_err(stringify)?;
    Ok(warning)
}

/// Returns a quick collection-level health summary for the splash screen.
#[tauri::command]
pub async fn collection_info(
    state: State<'_, AppStateArc>,
) -> Result<serde_json::Value, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let info = qdrant
        .collection_info(COLLECTION)
        .await
        .map_err(stringify)?;
    let r = info.result.unwrap_or_default();
    Ok(serde_json::json!({
        "collection": COLLECTION,
        "points_count": r.points_count.unwrap_or(0),
        "indexed_vectors_count": r.indexed_vectors_count.unwrap_or(0),
        "status": r.status,
        "segments_count": r.segments_count,
    }))
}

/// Lightweight scan/refresh — re-reads `~/.claude/projects` (modern),
/// `~/.codex/sessions` (P5 KH-01 multi-agent), AND `~/.claude/transcripts`
/// (legacy, pre-v2.1.114) and indexes anything new. Returns how many sessions
/// are now in the collection.
///
/// If `path` is provided, only that single root is scanned (overrides the
/// multi-root default). Useful for tests and CLI tools.
#[tauri::command]
pub async fn refresh_index(
    state: State<'_, AppStateArc>,
    path: Option<PathBuf>,
) -> Result<serde_json::Value, String> {
    // PERFORMANCE FIX (Gemini PR #5 review, commands.rs:332): the directory
    // walk + JSONL parse is synchronous and CPU-bound — running it on the
    // tokio worker froze the UI on large corpora. Offload to the blocking
    // pool so the webview stays responsive. The dispatcher must allocate
    // the closure inputs by `clone()` so the spawned task owns them.
    //
    // MERGE NOTE (full-sync): single-root path also pulls legacy
    // `~/.claude/transcripts/` so users with the legacy dir still benefit
    // from the upstream migration even when they override `path`.
    let sessions = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<parser::Session>> {
        if let Some(root) = path {
            let mut s = scan_root_routed(&root)?;
            if let Ok(legacy) = parser::scan_transcripts_dir(&default_transcripts_root()) {
                s.extend(legacy);
            }
            Ok(s)
        } else {
            scan_all_roots()
        }
    })
    .await
    .map_err(|e| format!("refresh_index parse task panicked: {e}"))?
    .map_err(stringify)?;
    let total = sessions.len();
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let embedder = state.embedder().await.map_err(stringify)?;
    indexer::ensure_collection(&qdrant).await.map_err(stringify)?;
    let report = indexer::bulk_index_arc(&qdrant, embedder, &sessions)
        .await
        .map_err(stringify)?;
    Ok(serde_json::json!({
        "indexed": report.indexed,
        "duplicates_skipped": report.duplicates_skipped,
        "errors": report.errors,
        "total_scanned": total,
    }))
}

/// Scan both `~/.claude/projects` AND `~/.codex/sessions` and return a unified
/// `Vec<Session>`. Missing roots are tolerated — if only one agent is
/// installed, we still get a usable corpus.
fn scan_all_roots() -> anyhow::Result<Vec<parser::Session>> {
    let mut all: Vec<parser::Session> = Vec::new();
    let claude_root = default_projects_root();
    if claude_root.exists() {
        match parser::scan_dir(&claude_root) {
            Ok(mut s) => all.append(&mut s),
            Err(e) => eprintln!("[memex] claude root scan: {e:#}"),
        }
    }
    let codex_root = default_codex_root();
    if codex_root.exists() {
        match crate::codex_parser::scan_codex_dir(&codex_root) {
            Ok(mut s) => all.append(&mut s),
            Err(e) => eprintln!("[memex] codex root scan: {e:#}"),
        }
    }
    // MERGE NOTE (full-sync): also fold in legacy `~/.claude/transcripts/`
    // so the upstream migration's older corpus (Anthropic stopped writing
    // here at v2.1.114) survives the multi-agent merge.
    let transcripts_root = default_transcripts_root();
    if transcripts_root.exists() {
        match parser::scan_transcripts_dir(&transcripts_root) {
            Ok(mut s) => all.append(&mut s),
            Err(e) => eprintln!("[memex] legacy transcripts scan: {e:#}"),
        }
    }
    if all.is_empty() {
        anyhow::bail!(
            "no sessions found — neither {} nor {} nor {} contained parseable rollouts",
            claude_root.display(),
            codex_root.display(),
            transcripts_root.display(),
        );
    }
    Ok(all)
}

/// Route a single explicit root to the right parser.
///
/// ROBUSTNESS FIX (Codex PR #5 review, commands.rs:383): the previous
/// implementation matched on the literal substring `/.codex/sessions`,
/// which silently fell back to the Claude parser whenever the user
/// pointed Memex at the same data via:
///   - a symlink (e.g. `~/codex_data -> ~/.codex/sessions`)
///   - an alternate mount path
///   - a different letter case (HFS+ case-insensitive volumes)
///
/// The Claude parser then accepts Codex JSONL but extracts almost no fields
/// from the alien schema, so refresh_index/list_sessions silently pollute
/// the index with empty sessions instead of surfacing a parse error. Fix:
/// canonicalize first and also peek at the first rollout filename pattern
/// to detect Codex content regardless of how the user reached the path.
fn scan_root_routed(root: &std::path::Path) -> anyhow::Result<Vec<parser::Session>> {
    // 1) Canonical path string — resolves symlinks, normalises case on HFS+.
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let s_lower = canonical.to_string_lossy().to_lowercase();
    if s_lower.contains("/.codex/sessions") || s_lower.ends_with(".codex/sessions") {
        return crate::codex_parser::scan_codex_dir(&canonical);
    }
    // 2) Content sniff — look at the first rollout-shaped file. Codex
    //    sessions follow the `rollout-*.jsonl` convention, while Claude's
    //    are typically `<uuid>.jsonl`. We only need to check ONE file to
    //    pick the right parser.
    let first_rollout = walkdir::WalkDir::new(&canonical)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| {
            let name = e.file_name().to_string_lossy().to_lowercase();
            name.starts_with("rollout-") && name.ends_with(".jsonl")
        });
    if first_rollout.is_some() {
        return crate::codex_parser::scan_codex_dir(&canonical);
    }
    parser::scan_dir(&canonical)
}

fn default_codex_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".codex");
        p.push("sessions");
        p
    } else {
        PathBuf::from(".codex/sessions")
    }
}

fn default_projects_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".claude");
        p.push("projects");
        p
    } else {
        PathBuf::from(".claude/projects")
    }
}

/// Legacy `~/.claude/transcripts/` directory — flat dir of `ses_*.jsonl`
/// files written by Claude Code before the silent rollout to
/// `~/.claude/projects/` around v2.1.114. Returning this alongside the
/// modern root is how Memex preserves the user's older 2–4 months of
/// corpus that Anthropic stopped writing into.
fn default_transcripts_root() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".claude");
        p.push("transcripts");
        p
    } else {
        PathBuf::from(".claude/transcripts")
    }
}

/// Unified scan over both the modern `projects/` tree and the legacy
/// `transcripts/` flat dir. Each root is tolerated independently so a
/// user who has only one of them (e.g. transcripts copied over from
/// another machine, or a fresh install where projects/ hasn't been
/// created yet) still gets a working dashboard. Only when BOTH roots
/// fail do we propagate the error.
///
/// MERGE NOTE (full-sync): superseded by `scan_all_roots` which also
/// includes Codex (`~/.codex/sessions`). Kept as a documented two-root
/// alternative and as an integration point for future tooling.
#[allow(dead_code)]
fn scan_all_sources() -> anyhow::Result<Vec<parser::Session>> {
    let projects_result = parser::scan_dir(&default_projects_root());
    let legacy_result = parser::scan_transcripts_dir(&default_transcripts_root());

    let mut out: Vec<parser::Session> = Vec::new();
    let projects_err = match projects_result {
        Ok(s) => {
            out.extend(s);
            None
        }
        Err(e) => Some(e),
    };
    // scan_transcripts_dir is already tolerant of a missing dir (returns
    // Ok(empty)); a real parse failure is the only way we land here.
    if let Ok(legacy) = legacy_result {
        out.extend(legacy);
    }

    if out.is_empty() {
        if let Some(e) = projects_err {
            return Err(e);
        }
    }
    Ok(out)
}

fn default_history_path() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let mut p = PathBuf::from(home);
        p.push(".claude");
        p.push("history.jsonl");
        p
    } else {
        PathBuf::from(".claude/history.jsonl")
    }
}

/// Read `~/.claude/history.jsonl` and return per-day prompt counts plus
/// the corpus span. Used as the *base layer* for the dashboard activity
/// heatmap so the timeline reflects the user's full 6–12 month working
/// history (history.jsonl survives the silent cleanup that wipes session
/// jsonls).
#[tauri::command]
pub async fn prompt_history_stats(
    path: Option<PathBuf>,
) -> Result<parser::PromptHistoryStats, String> {
    let p = path.unwrap_or_else(default_history_path);
    // Move the (potentially slow) ~25 k-line read off the IPC worker.
    let p2 = p.clone();
    tokio::task::spawn_blocking(move || parser::read_prompt_history_stats(&p2))
        .await
        .map_err(|e| format!("history stats task panicked: {e}"))?
        .map_err(stringify)
}

/// Lightweight session summary returned by `list_sessions` — no embeddings,
/// no Qdrant calls. Used by the Time Machine stack on app boot so the user
/// always sees something even before they type a query, and even if Qdrant
/// hasn't been indexed yet.
// SessionSummary moved to the Tauri-free `summary` module so the headless
// `web`/`mcp` builds can use it without the GUI. Re-exported here so existing
// `commands::SessionSummary` paths keep working in the GUI build.
pub use crate::summary::SessionSummary;

/// List sessions across `~/.claude/projects/` (modern), `~/.codex/sessions`
/// (P5 KH-01 multi-agent), AND `~/.claude/transcripts/` (legacy, pre-v2.1.114
/// flat dir) sorted most-recent first. Pure parser walk — independent of
/// Qdrant. Powers the Time Machine stack on app boot.
///
/// If `path` is provided, only that root is scanned (legacy transcripts still
/// folded in so the user keeps their full history).
#[tauri::command]
pub async fn list_sessions(
    path: Option<PathBuf>,
    limit: Option<usize>,
) -> Result<Vec<SessionSummary>, String> {
    // ~2 000 jsonl walks worth of file IO + JSON parsing — move it off
    // the tokio worker so other IPC requests (prompt_history_stats,
    // recall, …) aren't parked while we boot the dashboard.
    //
    // MERGE NOTE (full-sync): single-root path also pulls legacy
    // `~/.claude/transcripts/` so the upstream migration's full corpus
    // survives even when callers pin a specific root.
    let sessions = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<parser::Session>> {
        if let Some(root) = path {
            let mut s = scan_root_routed(&root)?;
            if let Ok(legacy) = parser::scan_transcripts_dir(&default_transcripts_root()) {
                s.extend(legacy);
            }
            Ok(s)
        } else {
            scan_all_roots()
        }
    })
    .await
    .map_err(|e| format!("list_sessions scan task panicked: {e}"))?
    .map_err(stringify)?;
    let mut sessions = sessions;
    sessions.sort_by(|a, b| b.start_time.cmp(&a.start_time));
    let limit = limit.unwrap_or(60).min(sessions.len());
    Ok(sessions
        .into_iter()
        .take(limit)
        .map(SessionSummary::from)
        .collect())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RecentError {
    pub session_id: String,
    pub project_name: String,
    pub error_text: String,
    pub source_path: String,
    pub seen_at_iso: String,
}

// P3: per-file (mtime, Option<RecentError>) cache so the 12 s polling tick
// only re-parses files whose mtime advanced since we last looked. Keyed by
// canonical path; never grows beyond the live file set so we don't need LRU.
struct TailCacheEntry {
    mtime: SystemTime,
    latest_err: Option<RecentError>,
}

static TAIL_CACHE: Lazy<Mutex<HashMap<PathBuf, TailCacheEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Phase 6 polling-style recall trigger. Walks `~/.claude/projects`, finds any
/// `*.jsonl` modified within `since_seconds`, re-parses, and surfaces the most
/// recent `tool_result.is_error` (or assistant-text "Error:" line). Frontend
/// polls every ~12 s; on hit it calls `recall(error_text)` and animates the
/// banner.
///
/// We trade real OS file watching for portability — polling is reliable, has
/// no permission edge cases, and on 80 sessions costs <50 ms per tick (and
/// closer to <10 ms once the mtime cache warms up).
#[tauri::command]
pub async fn tail_recent_errors(
    path: Option<PathBuf>,
    since_seconds: Option<u64>,
) -> Result<Vec<RecentError>, String> {
    use chrono::Utc;
    use walkdir::WalkDir;

    let root = path.unwrap_or_else(default_projects_root);
    let cutoff = SystemTime::now() - std::time::Duration::from_secs(since_seconds.unwrap_or(60));
    let now_iso = Utc::now().to_rfc3339();
    let mut out: Vec<RecentError> = Vec::new();

    for entry in WalkDir::new(&root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if p.components().any(|c| c.as_os_str() == "subagents") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        if modified < cutoff {
            continue;
        }

        // P3: cache hit — if mtime hasn't advanced, reuse the prior result.
        let path_buf = p.to_path_buf();
        let cached = {
            let cache = TAIL_CACHE.lock().expect("tail cache poisoned");
            cache.get(&path_buf).and_then(|e| {
                if e.mtime == modified {
                    e.latest_err.clone()
                } else {
                    None
                }
            })
        };
        if let Some(prev) = cached {
            // Refresh seen_at_iso so the frontend treats it as "still active".
            out.push(RecentError {
                seen_at_iso: now_iso.clone(),
                ..prev
            });
            continue;
        }

        let Ok(session) = parser::parse_session(p) else { continue };
        let mut latest_err: Option<String> = None;
        for turn in session.turns.iter().rev().take(6) {
            if latest_err.is_some() {
                break;
            }
            // ROOT-CAUSE FIX (D-13 user complaint about noisy recall banner
            // for jq syntax errors): only structured tool_result errors
            // surface to the banner. The previous code ALSO matched any
            // text line containing "error:" / "traceback" / "panic" —
            // including stderr passthrough from one-off shell commands
            // ("jq: error: syntax error", "rg: no match", "cat: file not
            // found"), which are seldom actionable as a recall hint and
            // simply add noise. Structured tool_results with `is_error:
            // true` come from Claude's tool-use protocol and have already
            // been validated as a real failure by the tool runtime; those
            // ARE worth surfacing.
            if let Some(err) = turn.tool_results.iter().rev().find(|r| r.is_error) {
                let head: String = err.content.chars().take(800).collect();
                // Filter out shell-stderr passthrough that wraps trivial
                // CLI quoting issues. These are non-actionable noise —
                // they reflect a typo, not a recurring bug.
                let lower = head.to_ascii_lowercase();
                let is_shell_stderr_noise = lower.starts_with("exit code")
                    || lower.contains("syntax error")
                    || lower.contains("command not found")
                    || lower.contains("no such file or directory")
                    || lower.contains("unbound variable")
                    || lower.contains("parse error");
                if is_shell_stderr_noise {
                    // Skip silently — keep looking through older turns for
                    // a real (non-shell-noise) tool error.
                    continue;
                }
                latest_err = Some(head);
                break;
            }
            // INTENTIONALLY: no body-text regex. Plain `error:` in user
            // prose ("I'm getting an error: …") is the user asking a
            // question, not the runtime encountering one — surfacing
            // recall banners for those was confusing.
        }

        let entry_err = latest_err.map(|err| RecentError {
            session_id: session.session_id,
            project_name: session.project_name.unwrap_or_default(),
            error_text: err,
            source_path: p.to_string_lossy().to_string(),
            seen_at_iso: now_iso.clone(),
        });

        // Update cache regardless (negative caching matters — files without
        // errors stay cheap on subsequent ticks).
        if let Ok(mut cache) = TAIL_CACHE.lock() {
            cache.insert(
                path_buf,
                TailCacheEntry {
                    mtime: modified,
                    latest_err: entry_err.clone(),
                },
            );
        }
        if let Some(ev) = entry_err {
            out.push(ev);
        }
    }
    Ok(out)
}

// ===========================================================================
// P4 advanced retrieval commands
// ===========================================================================

/// KB-03 — Mix & Match with explicit context pairs (Discovery API). The
/// legacy `mix_match(positive, negative, limit)` command stays in place for
/// backward compatibility; this new command exposes the pair-based shape.
#[tauri::command]
pub async fn mix_match_with_pairs(
    state: State<'_, AppStateArc>,
    target_session_id: String,
    pairs: Vec<crate::retrieval::ContextPair>,
    limit: Option<u64>,
) -> Result<Vec<crate::indexer::SearchHit>, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    crate::retrieval::mix_match_with_pairs(
        &qdrant,
        &target_session_id,
        &pairs,
        limit.unwrap_or(20),
    )
    .await
    .map_err(stringify)
}

/// KB-05 — Scroll v3 with order_by. Backward-compat: when `order_by` is
/// `None`, defaults to `start_ts_dt desc` so existing UI flows that rely on
/// "most recent first" semantics still work.
#[tauri::command]
pub async fn list_sessions_ordered(
    state: State<'_, AppStateArc>,
    order_by: Option<crate::retrieval::OrderBySpec>,
    limit: Option<u32>,
) -> Result<Vec<crate::retrieval::SessionMeta>, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    crate::retrieval::list_sessions_ordered(&qdrant, order_by, None, limit.unwrap_or(60))
        .await
        .map_err(stringify)
}

/// KA-03 — Lens search with optional group_by. When `group_by` is `None` the
/// response carries only `flat` hits (backward-compat). When provided, the
/// response also carries the `groups` projection.
#[tauri::command]
pub async fn lens_search_grouped(
    state: State<'_, AppStateArc>,
    query: String,
    group_by: Option<crate::retrieval::GroupBy>,
    limit: Option<u64>,
) -> Result<crate::retrieval::LensSearchResponse, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let embedder = state.embedder().await.map_err(stringify)?;
    crate::retrieval::lens_search_grouped(
        &qdrant,
        &embedder,
        &query,
        group_by,
        limit.unwrap_or(20),
    )
    .await
    .map_err(stringify)
}

/// KA-04 — RelevanceFeedback re-ranking. Caller supplies the previous query
/// text (re-embedded server-side so the IPC stays narrow) and the
/// positive/negative session IDs for binary feedback.
#[tauri::command]
pub async fn relevance_feedback(
    state: State<'_, AppStateArc>,
    previous_query: String,
    positive_ids: Vec<String>,
    negative_ids: Vec<String>,
    limit: Option<u64>,
) -> Result<Vec<crate::indexer::SearchHit>, String> {
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let embedder = state.embedder().await.map_err(stringify)?;
    crate::retrieval::relevance_feedback(
        &qdrant,
        &embedder,
        &positive_ids,
        &negative_ids,
        &previous_query,
        limit.unwrap_or(20),
    )
    .await
    .map_err(stringify)
}

/// P2 KA-01/02/05 + KB-02 — FormulaQuery-backed lens with per-vector score
/// breakdown. The richer `LensResult` shape replaces `SearchHit` for callers
/// (e.g. WOW-3 inspector) that want contribution bars, recency factor, and
/// has_errors boost. The legacy `lens_search` command still works and now
/// internally routes through the same FormulaQuery path.
#[tauri::command]
pub async fn lens_search_v2(
    state: State<'_, AppStateArc>,
    query: String,
    weights: Option<crate::lens::LensWeights>,
    limit: Option<u64>,
) -> Result<Vec<crate::lens::LensResult>, String> {
    let weights = weights.unwrap_or_default();
    let limit = limit.unwrap_or(20);
    let qdrant = state.qdrant().await.map_err(stringify)?;
    let embedder = state.embedder().await.map_err(stringify)?;
    crate::lens::lens_search_v2(&qdrant, &embedder, &query, &weights, limit)
        .await
        .map_err(stringify)
}
