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
    /// Cold-start memory primers composed and emitted this tick.
    pub primers_fired: usize,
    /// Loop-Breaker suggestions surfaced this tick.
    pub loops_broken: usize,
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

/// Cold-start primer minimum source-session threshold. Below this we stay
/// silent — a one-source primer just echoes the new session itself back.
const PRIMER_MIN_SOURCES: usize = 2;
/// Cold-start primer debounce per session_id — fire at most once per
/// session even if the watcher re-visits it on many ticks.
const PRIMER_DEBOUNCE: Duration = Duration::from_secs(60 * 60 * 24);
/// How many past sessions to mine for the cold-start primer.
const PRIMER_LIMIT: usize = 8;

/// **Loop Breaker** — active-session "stuck" detection thresholds.
///
/// The canonical definitions now live in the ungated `loopcheck` module so the
/// headless CLI surfaces (`memex loop-check`, `memex codex-notify`) can reach
/// them without pulling in Tauri. We re-export them here so existing callers
/// — including PR #7's `tests/loop_breaker_integration.rs`, which imports them
/// via `memex_lib::watcher::{…}` — keep compiling unchanged.
pub use crate::loopcheck::{
    error_count_in_window, is_stuck, LOOP_ERROR_THRESHOLD, LOOP_MIN_TURNS, LOOP_RECENT_WINDOW,
};
/// Per-session debounce so the watcher doesn't yell "you're stuck" every
/// tick while the user is still working. 20 minutes — long enough to
/// avoid noise, short enough to re-surface if a second loop kicks in.
const LOOP_DEBOUNCE: Duration = Duration::from_secs(60 * 20);

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
    // Companion cold-start primer: debounce per session_id (we don't want to
    // re-fire the primer every tick while the user is mid-session).
    let primer_debounce: Arc<AsyncMutex<HashMap<String, SystemTime>>> =
        Arc::new(AsyncMutex::new(HashMap::new()));
    // Loop Breaker: debounce per session_id with a short refractory so we
    // don't repeat the suggestion every tick once it's already shown.
    let loop_debounce: Arc<AsyncMutex<HashMap<String, SystemTime>>> =
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
            let stats = match tick(
                &state,
                &app,
                &root,
                &mtimes,
                &debounce,
                &predict_debounce,
                &primer_debounce,
                &loop_debounce,
            )
            .await
            {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[memex] watcher tick failed: {e:#}");
                    continue;
                }
            };

            if stats.reindexed > 0
                || stats.new > 0
                || stats.notifications_fired > 0
                || stats.primers_fired > 0
                || stats.loops_broken > 0
            {
                eprintln!(
                    "[memex] watcher tick · checked={} new={} reindexed={} errors={} notifications={} primers={} loops={} ({} ms)",
                    stats.checked,
                    stats.new,
                    stats.reindexed,
                    stats.errors,
                    stats.notifications_fired,
                    stats.primers_fired,
                    stats.loops_broken,
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

#[allow(clippy::too_many_arguments)]
async fn tick(
    state: &AppStateArc,
    app: &AppHandle,
    root: &Path,
    mtimes: &Arc<AsyncMutex<HashMap<PathBuf, SystemTime>>>,
    debounce: &Arc<AsyncMutex<HashMap<(String, String), SystemTime>>>,
    predict_debounce: &Arc<AsyncMutex<HashMap<(String, String), SystemTime>>>,
    primer_debounce: &Arc<AsyncMutex<HashMap<String, SystemTime>>>,
    loop_debounce: &Arc<AsyncMutex<HashMap<String, SystemTime>>>,
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
    // for recall notifications. The `bool` is `is_new`: brand-new file this
    // tick (not just a re-modified one), used by the cold-start primer.
    let mut hot: Vec<(Session, bool)> = Vec::new();

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
                    hot.push((session, is_new));
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
    //    latest error and the predicted next-action. For brand-new
    //    sessions, also fire the cold-start memory primer (Companion).
    for (session, is_new) in hot {
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
        if is_new {
            match maybe_fire_primer(&qdrant, &embedder, app, primer_debounce, &session).await {
                Ok(true) => stats.primers_fired += 1,
                Ok(false) => {}
                Err(e) => eprintln!(
                    "[memex] watcher primer failed for {}: {:#}",
                    session.session_id, e
                ),
            }
        }
        // Loop Breaker is *not* gated on is_new — it watches every fresh
        // mtime of every active session and only fires when the recent
        // error count exceeds the threshold.
        match maybe_fire_loop_breaker(&qdrant, &embedder, app, loop_debounce, &session).await {
            Ok(true) => stats.loops_broken += 1,
            Ok(false) => {}
            Err(e) => eprintln!(
                "[memex] watcher loop-breaker failed for {}: {:#}",
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

/// **Cold-start primer** (Companion). Fires when the watcher sees a brand-
/// new session jsonl appear — i.e., the user just opened Claude Code in a
/// directory. We synthesize a memory primer for that directory, drop a
/// macOS notification, and emit a `companion-primer-ready` Tauri event
/// carrying the full primer JSON so the frontend Companion panel can
/// render it and let the user copy the markdown straight to their
/// clipboard / system prompt.
///
/// Conditions:
/// 1. Session is brand-new this tick (`is_new == true`) — re-modifications
///    of an existing session don't qualify as cold starts.
/// 2. Primer surfaces ≥ `PRIMER_MIN_SOURCES` past sessions — below that
///    we'd just echo the new session back at itself.
/// 3. Per-session debounce so a single jsonl appearing across multiple
///    polling cycles fires the primer only once.
async fn maybe_fire_primer(
    qdrant: &qdrant_client::Qdrant,
    embedder: &indexer::Embedder,
    app: &AppHandle,
    debounce: &Arc<AsyncMutex<HashMap<String, SystemTime>>>,
    session: &Session,
) -> anyhow::Result<bool> {
    // Need a cwd to prime against — older transcripts and Codex-style
    // sessions occasionally lack project_path; bail rather than guess.
    let Some(cwd) = session.project_path.clone() else {
        return Ok(false);
    };
    if cwd.is_empty() {
        return Ok(false);
    }

    // Debounce by session_id so the watcher doesn't re-fire the primer
    // every tick for the same long-running session.
    let key = session.session_id.clone();
    {
        let g = debounce.lock().await;
        let now = SystemTime::now();
        if let Some(prev) = g.get(&key) {
            if now.duration_since(*prev).map(|d| d < PRIMER_DEBOUNCE).unwrap_or(false) {
                return Ok(false);
            }
        }
    }

    // Compose, excluding the new session itself so it can't prime its own
    // (still-empty) decisions back into the result.
    let primer = crate::companion::compose_memory_primer_excluding(
        qdrant,
        Some(embedder),
        std::path::Path::new(&cwd),
        PRIMER_LIMIT,
        std::slice::from_ref(&session.session_id),
    )
    .await?;

    if primer.source_sessions.len() < PRIMER_MIN_SOURCES {
        return Ok(false);
    }

    // Reserve the debounce slot only now that we know we're firing.
    {
        let mut g = debounce.lock().await;
        g.insert(key, SystemTime::now());
    }

    let project = session.project_name.as_deref().unwrap_or("?");
    let title = "Memex Companion · primer ready";
    let body = format!(
        "{} · loaded memory from {} past session(s) ({} decisions, {} pitfalls)",
        project,
        primer.source_sessions.len(),
        primer.stats.decisions_extracted,
        primer.stats.pitfalls_extracted,
    );
    notify_system(app, title, &body);

    // The frontend Companion panel listens on this event and pops the
    // markdown into view. Carries the full primer so no extra IPC roundtrip
    // is needed when the user clicks the notification.
    let _ = app.emit(
        "companion-primer-ready",
        json!({
            "kind": "primer",
            "session_id": session.session_id,
            "project_name": project,
            "primer": primer,
        }),
    );

    Ok(true)
}

/// **Loop Breaker.** Watches the active session for "stuck" patterns —
/// most concretely, ≥`LOOP_ERROR_THRESHOLD` `tool_result.is_error` events
/// inside the most recent `LOOP_RECENT_WINDOW` turns. When that fires,
/// runs `predict_next_actions` on the session, finds what past-you did
/// to break out of a similar position, and emits a `loop-breaker-ready`
/// Tauri event the frontend turns into a banner.
///
/// Differs from `maybe_fire_predict` (which is proactive — "past-you
/// usually does X next") and `maybe_fire_recall` (single-error
/// reactive). Loop Breaker is the pattern-of-errors detector — the
/// "you've been stuck for 12 turns and burned 5 tool calls on the same
/// thing" surface.
async fn maybe_fire_loop_breaker(
    qdrant: &qdrant_client::Qdrant,
    embedder: &indexer::Embedder,
    app: &AppHandle,
    debounce: &Arc<AsyncMutex<HashMap<String, SystemTime>>>,
    session: &Session,
) -> anyhow::Result<bool> {
    // Pure min-turns + error-count-in-window gate, shared verbatim with the
    // headless `memex loop-check` CLI (loopcheck::is_stuck).
    let recent_count = match is_stuck(session) {
        Some(ctx) => ctx.recent_errors,
        None => return Ok(false),
    };

    // Debounce: in-memory map (per-watcher-process) AND the on-disk
    // touch-file used by the headless `memex loop-check` hook. Honoring
    // both means the GUI banner + the Claude Code hook can't double-fire
    // for the same session — whichever surface stamps first suppresses
    // the other for the 20-min window (PR #8 follow-up #2).
    //
    // This is a CHEAP pre-check: skip the expensive `predict_next_actions`
    // call if we already know a recent stamp exists. The actual atomic
    // reservation happens after predict succeeds, just before emit
    // (`try_reserve_fire`, see below) — fixes the watcher↔hook race
    // codex flagged on PR #9.
    let key = session.session_id.clone();
    {
        let g = debounce.lock().await;
        let now = SystemTime::now();
        if let Some(prev) = g.get(&key) {
            if now.duration_since(*prev).map(|d| d < LOOP_DEBOUNCE).unwrap_or(false) {
                return Ok(false);
            }
        }
    }
    if !crate::loopcheck::should_fire_for_session(&key) {
        return Ok(false);
    }

    // Ask predict for what past-you would do at this conversational
    // position. The pivot-walk inside predict_next_actions is exactly
    // the "find a similar stuck moment and look at what unstuck you"
    // primitive we want here.
    let ctx = match indexer::predict_next_actions(
        qdrant,
        embedder,
        &session.session_id,
        4, // last_n_turns — wider context for stuck detection
        4, // horizon
        10, // neighbors
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            // predict failing is non-fatal — Loop Breaker still has signal
            // value as a plain "you're stuck" nudge, but we want at least
            // ONE suggestion attached to make the banner actionable.
            eprintln!("[memex] loop-breaker predict failed: {e:#}");
            return Ok(false);
        }
    };
    if ctx.predictions.is_empty() {
        return Ok(false);
    }

    // ATOMIC reservation right before emit. Race-free against the
    // headless `memex loop-check` hook running concurrently — both can
    // pass the cheap pre-check above and the expensive predict call,
    // but only one wins the `O_CREAT | O_EXCL` claim. The loser returns
    // here without emitting (codex PR #9 C-1).
    if !crate::loopcheck::try_reserve_fire(&key) {
        return Ok(false);
    }
    {
        let mut g = debounce.lock().await;
        g.insert(key.clone(), SystemTime::now());
    }

    let top = &ctx.predictions[0];
    let project = session.project_name.as_deref().unwrap_or("?");
    let title = "Memex Loop Breaker · you've been stuck";
    let body = format!(
        "{} · {} errors in {} turns. Past-you ran `{}` next ({} · turn #{}).",
        project,
        recent_count,
        LOOP_RECENT_WINDOW,
        top.tool_name,
        top.from_session_project,
        top.from_turn_index,
    );
    notify_system(app, title, &body);

    let _ = app.emit(
        "loop-breaker-ready",
        json!({
            "kind": "loop_breaker",
            "from_session_id": session.session_id,
            "from_project": project,
            "from_turn_index": session.turns.len().saturating_sub(1),
            "recent_errors": recent_count,
            "recent_window": LOOP_RECENT_WINDOW,
            "suggestion": {
                "tool_name": top.tool_name,
                "example_input": top.example_input_summary,
                "from_session_id": top.from_session_id,
                "from_session_project": top.from_session_project,
                "from_turn_index": top.from_turn_index,
                "frequency": top.frequency,
                "confidence": top.confidence,
            },
            "predictions": ctx.predictions,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Session, Turn, TurnRole, ToolResult, EventCounts};
    use chrono::Utc;

    fn turn_with_errors(error_count: usize) -> Turn {
        let mut t = Turn {
            uuid: "u".into(),
            parent_uuid: None,
            timestamp: Some(Utc::now()),
            role: TurnRole::Assistant,
            is_sidechain: false,
            text: String::new(),
            tool_calls: vec![],
            tool_results: vec![],
        };
        for i in 0..error_count {
            t.tool_results.push(ToolResult {
                tool_use_id: format!("tu_{i}"),
                content: "error".into(),
                is_error: true,
            });
        }
        t
    }

    fn sess(turns: Vec<Turn>) -> Session {
        Session {
            session_id: "s".into(),
            source_path: std::path::PathBuf::from("/tmp/s.jsonl"),
            project_path: None,
            project_name: Some("test".into()),
            git_branch: None,
            claude_version: None,
            ai_title: None,
            start_time: None,
            end_time: None,
            turns,
            event_counts: EventCounts::default(),
        }
    }

    #[test]
    fn error_count_in_window_only_counts_recent() {
        // 3 errors in the FIRST turn (outside window), 2 errors in the LAST turn.
        let mut turns = vec![turn_with_errors(3)];
        for _ in 0..10 {
            turns.push(turn_with_errors(0));
        }
        turns.push(turn_with_errors(2));
        let s = sess(turns);
        // window=5 captures only the trailing block — should see 2.
        assert_eq!(error_count_in_window(&s, 5), 2);
        // window=15 captures everything — should see 5.
        assert_eq!(error_count_in_window(&s, 15), 5);
    }

    #[test]
    fn error_count_in_window_zero_when_no_errors() {
        let s = sess(vec![turn_with_errors(0); 6]);
        assert_eq!(error_count_in_window(&s, 10), 0);
    }

    #[test]
    fn error_count_in_window_handles_window_larger_than_session() {
        let s = sess(vec![turn_with_errors(1), turn_with_errors(2)]);
        // window 100 > n=2 → counts everything.
        assert_eq!(error_count_in_window(&s, 100), 3);
    }
}
