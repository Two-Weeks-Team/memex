//! Background auto-index daemon + proactive recall.
//!
//! Polls `~/.claude/projects` every `period` seconds. For every top-level
//! `*.jsonl` whose `mtime` advanced since we last looked, re-parse + upsert
//! into Qdrant. Emits a Tauri event `index-updated` with per-tick stats so
//! the frontend can light up a fade-in chip.
//!
//! Then, for each freshly-modified session, looks for the latest
//! `tool_result.is_error` turn. If `indexer::recall(error_text)` returns a
//! cross-session hit with score ≥ 0.65, fires a macOS notification and emits
//! `open-replay-from-notification` so the frontend can deep-link into the
//! past session's resolution. Debounced 1 h per (current_session_id,
//! error_text_prefix) pair so the same error doesn't pop a banner every tick.
//!
//! We poll instead of using `notify`/FSEvents to stay portable, avoid macOS
//! permission prompts, and dodge the duplicate-event firehose that comes
//! with editors writing temp files. On 80+ sessions one tick is well under
//! 100 ms once the mtime cache is warm.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::{NotificationExt, PermissionState};
use tokio::sync::Mutex as AsyncMutex;
use walkdir::WalkDir;

use crate::commands::AppStateArc;
use crate::indexer::{self, SearchHit};
use crate::parser::{self, Session};

/// Which jsonl schema a candidate path follows. The watcher polls both the
/// modern `~/.claude/projects/` tree and (if present) the legacy
/// `~/.claude/transcripts/` flat dir so a user's pre-v2.1.114 corpus
/// — which Anthropic stopped writing into without announcement — is still
/// indexed and surfaced through Memex.
#[derive(Debug, Clone, Copy)]
enum SourceKind {
    Modern,
    LegacyTranscript,
}

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

/// Per-tick stats payload emitted on the `index-updated` Tauri event.
#[derive(Debug, Clone, Default, Serialize)]
pub struct TickStats {
    pub checked: usize,
    pub reindexed: usize,
    pub new: usize,
    pub errors: usize,
    pub notifications_fired: usize,
    pub elapsed_ms: u128,
}

/// Recall hit score threshold — only fire notifications when the top
/// cross-session match is genuinely close.
const RECALL_SCORE_THRESHOLD: f32 = 0.65;
/// Debounce window per (session_id, error_prefix). Keeps the same error from
/// re-firing every tick.
const RECALL_DEBOUNCE: Duration = Duration::from_secs(60 * 60);
/// Only consider notifications for files modified within this window of "now".
/// On a cold start, this prevents a flood of recall pops for old errors.
const FRESH_ERROR_WINDOW: Duration = Duration::from_secs(60 * 5);

/// Predict-based notification threshold — top-1 must have frequency > 0.7
/// (i.e., past-you took this action in >70% of comparable conversational
/// positions) before we'll interrupt the user.
const PREDICT_FREQ_THRESHOLD: f32 = 0.7;
/// Debounce window per (session_id, predicted_tool) — 30 min.
const PREDICT_DEBOUNCE: Duration = Duration::from_secs(60 * 30);

/// Hard cap on indexes per tick — fastembed's ONNX runtime pegs every CPU
/// core when batching big embedding runs, which on a user's machine with
/// 1 900+ legacy transcripts produces ~700% CPU and a screaming fan for
/// hours. Cap so each tick does at most this many sessions and the worker
/// goes idle between ticks. A full corpus warm-up will take N/cap ticks
/// (e.g. 1 989 / 30 ≈ 67 ticks ≈ 67 min at period=60 s) but the machine
/// stays usable the whole time.
const MAX_INDEX_PER_TICK: usize = 30;

/// First-tick boot delay — wait this long after app start before doing
/// anything heavy, so the UI window has time to paint and the user isn't
/// surprised by a fan spike on launch.
const BOOT_DELAY_SECS: u64 = 30;

/// Spawn the background watcher. Returns immediately; the task runs until the
/// process exits.
pub fn start_watcher(
    state: AppStateArc,
    app: AppHandle,
    root: PathBuf,
    period: Duration,
) {
    let mtimes: Arc<AsyncMutex<HashMap<PathBuf, SystemTime>>> =
        Arc::new(AsyncMutex::new(HashMap::new()));
    let debounce: Arc<AsyncMutex<HashMap<(String, String), SystemTime>>> =
        Arc::new(AsyncMutex::new(HashMap::new()));
    let predict_debounce: Arc<AsyncMutex<HashMap<(String, String), SystemTime>>> =
        Arc::new(AsyncMutex::new(HashMap::new()));

    // Use Tauri's async runtime so the watcher works when spawned from
    // `setup()` — at that point tokio's reactor isn't yet installed on the
    // current thread, but Tauri's runtime (also tokio under the hood) is.
    tauri::async_runtime::spawn(async move {
        eprintln!(
            "[memex] watcher started · root={} · period={}s",
            root.display(),
            period.as_secs()
        );

        // Ask for notification permission once on startup. macOS shows the
        // prompt the first time; subsequent launches reuse the cached state.
        ensure_notification_permission(&app);

        // First tick: longer delay so the UI window gets to paint and the
        // user isn't met with a CPU spike during startup. After the first
        // tick we fall back to the configured period.
        let mut delay = Duration::from_secs(BOOT_DELAY_SECS);
        loop {
            tokio::time::sleep(delay).await;
            delay = period;

            if !root.exists() {
                continue;
            }

            let start = std::time::Instant::now();
            let stats = match tick(&state, &app, &root, &mtimes, &debounce, &predict_debounce).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[memex] watcher tick failed: {e:#}");
                    continue;
                }
            };

            if stats.reindexed > 0 || stats.new > 0 || stats.notifications_fired > 0 {
                eprintln!(
                    "[memex] watcher tick · checked={} new={} reindexed={} errors={} notifications={} ({} ms)",
                    stats.checked,
                    stats.new,
                    stats.reindexed,
                    stats.errors,
                    stats.notifications_fired,
                    start.elapsed().as_millis()
                );
                let _ = app.emit("index-updated", &stats);
            }
        }
    });
}

fn ensure_notification_permission(app: &AppHandle) {
    let n = app.notification();
    let state = match n.permission_state() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[memex] notification permission_state failed: {e:#}");
            return;
        }
    };
    match state {
        PermissionState::Granted => {
            eprintln!("[memex] notification permission: granted");
        }
        PermissionState::Denied => {
            eprintln!(
                "[memex] notification permission denied — enable in System Settings → Notifications → Memex to receive recall alerts"
            );
        }
        // Prompt | PromptWithRationale — ask now.
        _ => match n.request_permission() {
            Ok(s) => eprintln!("[memex] notification permission requested → {s:?}"),
            Err(e) => eprintln!("[memex] notification request_permission failed: {e:#}"),
        },
    }
}

async fn tick(
    state: &AppStateArc,
    app: &AppHandle,
    root: &Path,
    mtimes: &Arc<AsyncMutex<HashMap<PathBuf, SystemTime>>>,
    debounce: &Arc<AsyncMutex<HashMap<(String, String), SystemTime>>>,
    predict_debounce: &Arc<AsyncMutex<HashMap<(String, String), SystemTime>>>,
) -> anyhow::Result<TickStats> {
    let mut stats = TickStats::default();
    let started = std::time::Instant::now();

    // 1. Cheap walk over BOTH the modern projects/ tree and (if present)
    //    the legacy transcripts/ flat dir. Each candidate carries a flag
    //    telling us which parser to use later.
    let mut candidates: Vec<(PathBuf, SystemTime, SourceKind)> = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        if path.components().any(|c| c.as_os_str() == "subagents") {
            continue;
        }
        stats.checked += 1;
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else { continue };
        candidates.push((path.to_path_buf(), modified, SourceKind::Modern));
    }
    // Legacy transcripts/ — only if it exists. Flat dir, only ses_*.jsonl.
    let transcripts_root = default_transcripts_root();
    if transcripts_root.exists() {
        for entry in WalkDir::new(&transcripts_root)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if !stem.starts_with("ses_") {
                continue;
            }
            stats.checked += 1;
            let Ok(meta) = entry.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };
            candidates.push((path.to_path_buf(), modified, SourceKind::LegacyTranscript));
        }
    }

    // 2. Filter to files whose mtime is new-to-us OR advanced.
    let mut to_index: Vec<(PathBuf, SystemTime, bool, SourceKind)> = Vec::new();
    {
        let mtimes_guard = mtimes.lock().await;
        for (path, modified, kind) in &candidates {
            match mtimes_guard.get(path) {
                Some(prev) if *prev >= *modified => {}
                Some(_) => to_index.push((path.clone(), *modified, false, *kind)),
                None => to_index.push((path.clone(), *modified, true, *kind)),
            }
        }
    }

    if to_index.is_empty() {
        stats.elapsed_ms = started.elapsed().as_millis();
        return Ok(stats);
    }

    // Hard-cap batch size so a fresh-corpus warm-up doesn't peg the CPU
    // for hours. Remaining files get picked up on subsequent ticks.
    let backlog = to_index.len();
    if backlog > MAX_INDEX_PER_TICK {
        to_index.truncate(MAX_INDEX_PER_TICK);
        let remaining = backlog - MAX_INDEX_PER_TICK;
        eprintln!(
            "[memex] warm-up: indexing {MAX_INDEX_PER_TICK}/{backlog} this tick · {remaining} left for next ticks"
        );
    }

    // 3. Lazy-init the heavy state only when we know there's work.
    let qdrant = state.qdrant().await?;
    let embedder = state.embedder().await?;

    let mut to_remember: Vec<(PathBuf, SystemTime)> = Vec::with_capacity(to_index.len());
    // Sessions we just (re)indexed AND whose mtime is "fresh" — candidates
    // for recall notifications.
    let mut hot: Vec<Session> = Vec::new();

    let now = SystemTime::now();
    for (path, modified, is_new, kind) in to_index {
        let parse_result = match kind {
            SourceKind::Modern => parser::parse_session(&path),
            SourceKind::LegacyTranscript => parser::parse_transcript_session(&path),
        };
        let session = match parse_result {
            Ok(s) => s,
            Err(e) => {
                eprintln!("[memex] watcher parse_session failed for {}: {:#}", path.display(), e);
                stats.errors += 1;
                continue;
            }
        };
        match indexer::index_session(&qdrant, &embedder, &session).await {
            Ok(()) => {
                if is_new {
                    stats.new += 1;
                } else {
                    stats.reindexed += 1;
                }
                to_remember.push((path.clone(), modified));
                // Only fire recall for sessions whose mtime is genuinely
                // recent — avoids flooding on first launch.
                if now
                    .duration_since(modified)
                    .map(|d| d <= FRESH_ERROR_WINDOW)
                    .unwrap_or(false)
                {
                    hot.push(session);
                }
            }
            Err(e) => {
                eprintln!("[memex] watcher index_session failed for {}: {:#}", path.display(), e);
                stats.errors += 1;
            }
        }
    }

    // 4. Update the mtime cache atomically.
    if !to_remember.is_empty() {
        let mut mtimes_guard = mtimes.lock().await;
        for (path, modified) in to_remember {
            mtimes_guard.insert(path, modified);
        }
    }

    // 5. Proactive recall + predict — for each hot session, check the
    //    latest error and the predicted next-action.
    for session in hot {
        match maybe_fire_recall(&qdrant, &embedder, app, debounce, &session).await {
            Ok(true) => stats.notifications_fired += 1,
            Ok(false) => {}
            Err(e) => eprintln!(
                "[memex] watcher recall failed for {}: {:#}",
                session.session_id, e
            ),
        }
        match maybe_fire_predict(&qdrant, &embedder, app, predict_debounce, &session).await {
            Ok(true) => stats.notifications_fired += 1,
            Ok(false) => {}
            Err(e) => eprintln!(
                "[memex] watcher predict failed for {}: {:#}",
                session.session_id, e
            ),
        }
    }

    stats.elapsed_ms = started.elapsed().as_millis();
    Ok(stats)
}

/// Returns `Ok(true)` if a notification was fired.
async fn maybe_fire_recall(
    qdrant: &qdrant_client::Qdrant,
    embedder: &indexer::Embedder,
    app: &AppHandle,
    debounce: &Arc<AsyncMutex<HashMap<(String, String), SystemTime>>>,
    session: &Session,
) -> anyhow::Result<bool> {
    let Some((turn_index, error_text)) = latest_error(session) else {
        return Ok(false);
    };

    // Debounce key — first 120 chars are enough to disambiguate near-duplicates
    // without thrashing the HashMap on noisy stack traces.
    let prefix: String = error_text.chars().take(120).collect();
    let key = (session.session_id.clone(), prefix.clone());
    {
        let g = debounce.lock().await;
        let now = SystemTime::now();
        if let Some(prev) = g.get(&key) {
            if now.duration_since(*prev).map(|d| d < RECALL_DEBOUNCE).unwrap_or(false) {
                return Ok(false);
            }
        }
        // Don't insert yet — only mark as "recently notified" once an
        // alert actually fires. If we set the key here and recall has 0
        // cross-session hits, a *future* match (e.g. user solves the same
        // error in another session and that session gets indexed) would
        // be silenced for the next hour for no reason.
    }

    let hits = indexer::recall(qdrant, embedder, &error_text, 5).await?;
    // Drop self-matches + threshold.
    let cross: Vec<SearchHit> = hits
        .into_iter()
        .filter(|h| h.session_id != session.session_id && h.score >= RECALL_SCORE_THRESHOLD)
        .collect();
    if cross.is_empty() {
        return Ok(false);
    }

    // Reserve the debounce slot only now that we know we're about to fire.
    {
        let mut g = debounce.lock().await;
        g.insert(key, SystemTime::now());
    }

    let top = &cross[0];
    let project = session.project_name.as_deref().unwrap_or("?");
    let title = "Memex · I've seen this error before";
    let body = format!(
        "{} · turn #{}  ·  match {:.0}% in {}",
        project,
        turn_index,
        top.score * 100.0,
        if top.ai_title.is_empty() { top.project_name.as_str() } else { top.ai_title.as_str() }
    );

    notify_system(app, title, &body);

    // Emit deep-link event so the frontend can auto-open the replay of the
    // matched past session whenever the main window comes into focus.
    // turn_index defaults to 0 (start of past session) — the user scrubs
    // forward via the existing Time Machine controls.
    let _ = app.emit(
        "open-replay-from-notification",
        json!({
            "kind": "recall",
            "from_session_id": session.session_id,
            "from_turn_index": turn_index,
            "from_project": project,
            "error_text": error_text,
            "session_id": top.session_id,
            "turn_index": 0,
            "match_score": top.score,
            "match_project": top.project_name,
            "match_title": top.ai_title,
            "cross_hits": cross.iter().map(|h| json!({
                "session_id": h.session_id,
                "project_name": h.project_name,
                "ai_title": h.ai_title,
                "score": h.score,
            })).collect::<Vec<_>>()
        }),
    );

    Ok(true)
}

/// Predict-based notification (IMPL-MCP T3.7) — "past-you ran &lt;tool&gt;
/// next 80 % of times". Returns `Ok(true)` if a notification was fired.
///
/// Conditions:
/// 1. The live session has at least 3 turns (we need a conversational
///    position to neighbor-match against).
/// 2. `predict_next_actions(...)` returns at least one prediction with
///    `frequency > 0.7`.
/// 3. The live session has NOT YET called that tool — i.e., the prediction
///    is genuinely a *next* step, not something past-you and present-you
///    already did.
async fn maybe_fire_predict(
    qdrant: &qdrant_client::Qdrant,
    embedder: &indexer::Embedder,
    app: &AppHandle,
    debounce: &Arc<AsyncMutex<HashMap<(String, String), SystemTime>>>,
    session: &Session,
) -> anyhow::Result<bool> {
    if session.turns.len() < 3 {
        return Ok(false);
    }

    let ctx = indexer::predict_next_actions(qdrant, embedder, &session.session_id, 3, 3, 8).await?;
    let Some(top) = ctx.predictions.first() else {
        return Ok(false);
    };
    if top.frequency <= PREDICT_FREQ_THRESHOLD {
        return Ok(false);
    }

    // Filter: the live session must not have already called this tool.
    let already_called = session
        .turns
        .iter()
        .flat_map(|t| t.tool_calls.iter())
        .any(|tc| tc.name.eq_ignore_ascii_case(&top.tool_name));
    if already_called {
        return Ok(false);
    }

    // Debounce: 30 min per (session_id, lowercased tool_name) — match the
    // case-insensitive `already_called` check above so the two filters
    // agree on what "the same tool" means.
    let key = (session.session_id.clone(), top.tool_name.to_ascii_lowercase());
    {
        let g = debounce.lock().await;
        let now = SystemTime::now();
        if let Some(prev) = g.get(&key) {
            if now.duration_since(*prev).map(|d| d < PREDICT_DEBOUNCE).unwrap_or(false) {
                return Ok(false);
            }
        }
        // Reserve below, only after we commit to actually firing.
    }

    let pct = (top.frequency * 100.0).round() as i32;
    let title = "Memex · past-you would do this next";
    let body = format!(
        "{}  ·  {} ran {} next {}% of times",
        session.project_name.as_deref().unwrap_or("?"),
        top.from_session_project,
        top.tool_name,
        pct
    );
    notify_system(app, title, &body);
    {
        let mut g = debounce.lock().await;
        g.insert(key, SystemTime::now());
    }

    let _ = app.emit(
        "open-replay-from-notification",
        json!({
            "from_session_id": session.session_id,
            "from_turn_index": session.turns.len().saturating_sub(1),
            "from_project": session.project_name.clone().unwrap_or_default(),
            "kind": "predict",
            "session_id": top.from_session_id,
            "turn_index": top.from_turn_index,
            "match_project": top.from_session_project,
            "match_title": top.tool_name,
            "match_score": top.confidence,
            "predicted_tool": top.tool_name,
            "predicted_freq": top.frequency,
            "example_input": top.example_input_summary,
        }),
    );

    Ok(true)
}

/// Surface a system notification. We go exclusively through
/// `tauri-plugin-notification`. The earlier osascript fallback worked but
/// every notification fired through `osascript display notification` is
/// owned by Script Editor in macOS's eyes — clicking it activates Script
/// Editor instead of Memex, which defeats the deep-link UX. Through the
/// plugin the notification is owned by Memex.app (LaunchServices already
/// has it registered with `activityTypes: NOTIFICATION#:dev.sgwannabe.memex`)
/// so a click activates Memex and the frontend's existing
/// `open-replay-from-notification` listener picks up the deep-link.
fn notify_system(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
    {
        eprintln!("[memex] notification show failed: {e:#}");
    }
}

/// Walk the session's last few turns and return the most recent error
/// (turn_index, error_text). Mirrors `commands::tail_recent_errors` logic so
/// the watcher and the in-app banner agree on what counts as an "error".
fn latest_error(session: &Session) -> Option<(usize, String)> {
    let n = session.turns.len();
    for (rev_i, turn) in session.turns.iter().rev().take(6).enumerate() {
        let idx = n - 1 - rev_i;
        if let Some(err) = turn.tool_results.iter().rev().find(|r| r.is_error) {
            let head: String = err.content.chars().take(800).collect();
            return Some((idx, head));
        }
        for line in turn.text.lines().rev() {
            let lower = line.to_ascii_lowercase();
            if lower.contains("error:") || lower.contains("traceback") || lower.contains("panic") {
                return Some((idx, line.trim().to_string()));
            }
        }
    }
    None
}
