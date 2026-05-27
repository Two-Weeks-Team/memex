//! **Loop Breaker — headless stuck-detection core.**
//!
//! The "you've been stuck for N turns burning tool calls on the same thing"
//! detector originally lived inside `watcher.rs`, which is `#[cfg(feature =
//! "gui")]` (it pulls in Tauri's `AppHandle` / notification plugin). The
//! agent-integration CLI surfaces (`memex loop-check`, `memex codex-notify`)
//! run under the **headless** `web` build and on the plain CLI binary, so the
//! thresholds and the pure detection gate have to be reachable without any
//! Tauri dependency.
//!
//! This module is therefore declared **ungated** in `lib.rs` and holds:
//!   - the `pub const LOOP_*` thresholds (single source of truth), and
//!   - [`is_stuck`], a pure function that applies the same gate
//!     `watcher::maybe_fire_loop_breaker` uses (min-turns + error-count-in-
//!     window) with **zero** Tauri / notification / Qdrant dependencies.
//!
//! `watcher` re-exports the consts (`pub use crate::loopcheck::{…}`) so PR #7's
//! `tests/loop_breaker_integration.rs` — which imports them via
//! `memex_lib::watcher` — keeps compiling unchanged.

use crate::parser::Session;

/// **Loop Breaker** — active-session "stuck" detection thresholds.
/// Trigger when the user has fired ≥ `LOOP_ERROR_THRESHOLD` tool errors in the
/// most recent `LOOP_RECENT_WINDOW` turns AND the session has at least
/// `LOOP_MIN_TURNS` turns total (so we have enough history to look at).
///
/// Exposed `pub` so integration tests and tooling can assert against the same
/// threshold production uses — prevents the drift Quality review flagged (M2).
pub const LOOP_MIN_TURNS: usize = 12;
/// Window of most-recent turns the error-count gate looks at.
pub const LOOP_RECENT_WINDOW: usize = 10;
/// Errors-in-window threshold that flips the banner.
pub const LOOP_ERROR_THRESHOLD: usize = 3;

/// Per-session debounce window for Loop Breaker hook emission. Once the
/// hook surfaces the pivot for a given session_id, suppress further emits
/// for this window so the agent doesn't see the same `# ⚠ Memex Loop
/// Breaker` block on every subsequent Bash error in the same loop.
///
/// 20 minutes mirrors the GUI watcher's `LOOP_DEBOUNCE` so the two
/// surfaces (Tauri banner + hook injection) honor the same cool-down
/// window — addresses PR #8 follow-up #2 (Loop Breaker dual-fire).
pub const LOOP_DEBOUNCE_SECS: u64 = 60 * 20;

/// Why a session was judged "stuck", carrying the numbers the pivot surface
/// needs to render an honest, factual message ("N errors in the last W turns").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StuckContext {
    /// `tool_result.is_error=true` events counted in the most recent
    /// `LOOP_RECENT_WINDOW` turns.
    pub recent_errors: usize,
    /// The window size the count was taken over (= `LOOP_RECENT_WINDOW`).
    pub recent_window: usize,
    /// Total turns in the session at detection time.
    pub total_turns: usize,
}

/// Count `tool_result.is_error=true` events in the most recent `window` turns
/// of `session`. O(window) — the same primitive Loop Breaker uses in the GUI
/// watcher (kept here as the canonical implementation).
pub fn error_count_in_window(session: &Session, window: usize) -> usize {
    let n = session.turns.len();
    let start = n.saturating_sub(window);
    session.turns[start..]
        .iter()
        .flat_map(|t| t.tool_results.iter())
        .filter(|r| r.is_error)
        .count()
}

/// Pure stuck-detection gate, free of any Tauri / Qdrant / notification
/// dependency. Returns `Some(StuckContext)` when BOTH gates pass:
///   1. `session.turns.len() >= LOOP_MIN_TURNS`
///   2. `error_count_in_window(session, LOOP_RECENT_WINDOW) >= LOOP_ERROR_THRESHOLD`
///
/// Otherwise `None`. This is exactly the condition
/// `watcher::maybe_fire_loop_breaker` checks before it consults `predict`.
pub fn is_stuck(session: &Session) -> Option<StuckContext> {
    if session.turns.len() < LOOP_MIN_TURNS {
        return None;
    }
    let recent_errors = error_count_in_window(session, LOOP_RECENT_WINDOW);
    if recent_errors < LOOP_ERROR_THRESHOLD {
        return None;
    }
    Some(StuckContext {
        recent_errors,
        recent_window: LOOP_RECENT_WINDOW,
        total_turns: session.turns.len(),
    })
}

// ---------------------------------------------------------------------------
// Cross-process debounce (PR #8 follow-up #2)
// ---------------------------------------------------------------------------
//
// The hook command (`memex loop-check --hook post-tool-use`) is a one-shot
// process spawned by Claude Code on every Bash PostToolUse — there's no
// in-process state to hold a debounce HashMap (unlike the long-lived GUI
// watcher). Use a touch-file per session whose mtime IS the last-fired
// timestamp. The GUI watcher honors the same path, so a primer fired by the
// watcher also suppresses the hook (and vice versa) for the next 20 min.

/// Returns the directory where Loop Breaker debounce touch-files live.
/// `$XDG_CACHE_HOME/memex/loopcheck` on Unix, `%LOCALAPPDATA%\memex\loopcheck`
/// on Windows. Creates the directory if it doesn't exist.
/// Resolve the debounce directory path WITHOUT creating it. Read-only —
/// `should_fire_for_session` runs on the hook hot path (every Bash
/// PostToolUse), so we don't want a `create_dir_all` syscall there.
/// Creation is deferred to `mark_fired_for_session` (the write path).
///
/// When `dirs::cache_dir()` returns `None` (no XDG / no platform cache),
/// fall back to a **user-scoped** subdirectory under `temp_dir()` so
/// multiple users on the same host don't collide on `/tmp/memex/loopcheck`
/// (where the FIRST user to write owns the dir and subsequent users hit
/// EACCES). Suggested by gemini-code-assist on PR #9.
fn debounce_dir() -> std::path::PathBuf {
    if let Some(base) = dirs::cache_dir() {
        return base.join("memex").join("loopcheck");
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "default".to_string());
    std::env::temp_dir()
        .join(format!("memex-{user}"))
        .join("loopcheck")
}

/// Sanitize a session_id into a filename-safe stem. session_ids are
/// already UUID-shaped in practice, but be defensive against arbitrary
/// agent identifiers.
fn debounce_stem(session_id: &str) -> String {
    session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
        .take(96)
        .collect()
}

fn debounce_path(session_id: &str) -> std::path::PathBuf {
    debounce_dir().join(format!("{}.ts", debounce_stem(session_id)))
}

/// True iff Loop Breaker hasn't fired for this session within
/// `LOOP_DEBOUNCE_SECS`. Reading the touch-file's mtime is a single stat
/// syscall — fast on the hook hot path. Returns `true` on any I/O error
/// (fail-open: better to occasionally double-emit than to suppress
/// indefinitely if the cache dir is unwriteable).
pub fn should_fire_for_session(session_id: &str) -> bool {
    let path = debounce_path(session_id);
    let Ok(meta) = std::fs::metadata(&path) else {
        return true;
    };
    let Ok(mtime) = meta.modified() else { return true };
    let Ok(elapsed) = std::time::SystemTime::now().duration_since(mtime) else {
        return true;
    };
    elapsed.as_secs() >= LOOP_DEBOUNCE_SECS
}

/// Stamp the touch-file so future calls within the debounce window
/// suppress further emits. Best-effort: I/O failure is logged-but-ignored
/// so a missing stamp doesn't break the hook output (which the agent has
/// already received by the time we mark).
///
/// **Race-prone**: this is a non-atomic "create or truncate". Two
/// processes both calling `should_fire → predict → mark_fired` can each
/// pass the pre-check, both fire, then both stamp. Use
/// [`try_reserve_fire`] instead when the caller is on the critical path
/// that decides emit vs. skip (PR #9 codex C-1).
pub fn mark_fired_for_session(session_id: &str) {
    let path = debounce_path(session_id);
    // Ensure the cache dir exists before we try to write — `debounce_dir`
    // no longer eagerly creates it on the read path (gemini PR #9 G-1).
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // `File::create` with truncate=true updates the mtime even when the
    // file already exists. Empty payload is intentional — mtime IS the
    // signal.
    if let Err(e) = std::fs::write(&path, b"") {
        eprintln!(
            "[memex loopcheck] failed to write debounce stamp at {}: {e}",
            path.display()
        );
    }
}

/// **Atomic check-and-reserve** — the race-free primitive callers should
/// use right before emit. Returns `true` when this caller won the
/// reservation (must emit); `false` when another process reserved it OR
/// a recent stamp still suppresses (must skip).
///
/// The two surfaces that share the Loop Breaker (GUI watcher and the
/// hook one-shot) each do an expensive `predict_next_actions` call
/// between the cheap `should_fire_for_session` pre-check and the actual
/// emit. Without an atomic reserve there's a check-time/use-time window:
/// both pass `should_fire`, both predict in parallel, both `mark_fired`,
/// both emit. (codex PR #9 C-1.)
///
/// Semantics:
///   - If a stamp exists and its mtime is within `LOOP_DEBOUNCE_SECS` →
///     return `false` (someone else reserved recently).
///   - Else: remove any stale stamp (older than debounce) and attempt
///     `OpenOptions::create_new` — only one process can succeed; the
///     loser sees `AlreadyExists` and returns `false`.
///
/// On any I/O error (cache dir unwriteable, permissions, etc.) we
/// fail-CLOSED here: return `false` so we don't double-emit. This is the
/// inverse of the fail-open posture in [`should_fire_for_session`] — at
/// the emit boundary, "skip" is the safer default than "fire."
pub fn try_reserve_fire(session_id: &str) -> bool {
    let path = debounce_path(session_id);

    // Phase 1: peek at any existing stamp. If it's fresh, refuse.
    if let Ok(meta) = std::fs::metadata(&path) {
        if let Ok(mtime) = meta.modified() {
            if let Ok(elapsed) = std::time::SystemTime::now().duration_since(mtime) {
                if elapsed.as_secs() < LOOP_DEBOUNCE_SECS {
                    return false;
                }
            }
        }
        // Stale stamp (older than debounce window) — remove so the next
        // `create_new` can succeed.
        let _ = std::fs::remove_file(&path);
    }

    // Phase 2: ensure parent dir exists (write path responsibility — see
    // gemini PR #9 G-1/G-2).
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Phase 3: atomic claim. `create_new(true)` translates to `O_CREAT |
    // O_EXCL` on Unix and the equivalent on Windows — only one process
    // wins on a given path.
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{EventCounts, Session, ToolResult, Turn, TurnRole};

    fn turn_with_errors(error_count: usize) -> Turn {
        let mut t = Turn {
            uuid: "u".into(),
            parent_uuid: None,
            timestamp: None,
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
        let mut turns = vec![turn_with_errors(3)];
        for _ in 0..10 {
            turns.push(turn_with_errors(0));
        }
        turns.push(turn_with_errors(2));
        let s = sess(turns);
        assert_eq!(error_count_in_window(&s, 5), 2);
        assert_eq!(error_count_in_window(&s, 15), 5);
    }

    #[test]
    fn is_stuck_none_when_too_few_turns() {
        // 5 turns, all errors — but below LOOP_MIN_TURNS so we stay silent.
        let s = sess(vec![turn_with_errors(2); 5]);
        assert!(is_stuck(&s).is_none());
    }

    #[test]
    fn is_stuck_none_when_few_errors() {
        // Plenty of turns, but the recent window has too few errors.
        let mut turns = vec![turn_with_errors(0); LOOP_MIN_TURNS + 5];
        // 1 error in the last window — below LOOP_ERROR_THRESHOLD.
        *turns.last_mut().unwrap() = turn_with_errors(1);
        let s = sess(turns);
        assert!(is_stuck(&s).is_none());
    }

    #[test]
    fn is_stuck_fires_on_error_burst() {
        // 14 turns total; the trailing 3 carry one error each → 3 in window.
        let mut turns = vec![turn_with_errors(0); LOOP_MIN_TURNS - 1];
        for _ in 0..LOOP_ERROR_THRESHOLD {
            turns.push(turn_with_errors(1));
        }
        let s = sess(turns);
        let ctx = is_stuck(&s).expect("should be stuck");
        assert_eq!(ctx.recent_errors, LOOP_ERROR_THRESHOLD);
        assert_eq!(ctx.recent_window, LOOP_RECENT_WINDOW);
        assert_eq!(ctx.total_turns, s.turns.len());
    }

    #[test]
    fn is_stuck_boundary_exact_thresholds() {
        // Exactly LOOP_MIN_TURNS turns; exactly LOOP_ERROR_THRESHOLD errors in
        // the window → fires (>= is inclusive).
        let mut turns = vec![turn_with_errors(0); LOOP_MIN_TURNS - LOOP_ERROR_THRESHOLD];
        for _ in 0..LOOP_ERROR_THRESHOLD {
            turns.push(turn_with_errors(1));
        }
        assert_eq!(turns.len(), LOOP_MIN_TURNS);
        let s = sess(turns);
        assert!(is_stuck(&s).is_some());
    }

    // ----- PR #8 follow-up #2 — cross-process debounce ----------------

    /// Sanitization: turn `/`, `.`, and other path-busting chars into `_`
    /// so a malformed session id can't traverse out of the debounce dir.
    #[test]
    fn debounce_stem_strips_path_traversal_chars() {
        let s = debounce_stem("../../etc/passwd");
        assert!(!s.contains('/'));
        assert!(!s.contains('.'));
        assert!(s.starts_with('_'));
    }

    #[test]
    fn debounce_stem_caps_length() {
        let huge = "a".repeat(500);
        assert!(debounce_stem(&huge).len() <= 96);
    }

    #[test]
    fn debounce_should_fire_then_marked_suppresses() {
        // Use a unique session id per test run to avoid colliding with
        // any real debounce file the local watcher might have written.
        let sid = format!(
            "memex-test-debounce-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        // Before marking: fire is allowed.
        assert!(should_fire_for_session(&sid));
        // After marking: fire is suppressed (mtime is now-ish).
        mark_fired_for_session(&sid);
        assert!(
            !should_fire_for_session(&sid),
            "freshly-marked session should suppress further fires within debounce window"
        );
        // Cleanup the touch-file so we don't leak test state.
        let _ = std::fs::remove_file(debounce_path(&sid));
    }

    #[test]
    fn debounce_should_fire_when_no_touch_file_exists() {
        // Fail-open: a session we've never marked is always allowed to fire.
        let sid = format!(
            "memex-test-debounce-virgin-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        // Belt-and-suspenders: ensure no stale file from a prior run.
        let _ = std::fs::remove_file(debounce_path(&sid));
        assert!(should_fire_for_session(&sid));
    }

    // ----- PR #9 codex C-1 — atomic check-and-reserve --------------

    #[test]
    fn try_reserve_fire_wins_first_call_blocks_subsequent() {
        let sid = format!(
            "memex-test-reserve-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let _ = std::fs::remove_file(debounce_path(&sid));

        // First call wins atomically.
        assert!(
            try_reserve_fire(&sid),
            "first try_reserve_fire on a fresh session should win"
        );
        // Subsequent calls within the debounce window lose — this is the
        // primitive that fixes the watcher↔hook race window.
        assert!(
            !try_reserve_fire(&sid),
            "second try_reserve_fire within debounce must lose"
        );
        assert!(
            !try_reserve_fire(&sid),
            "third try_reserve_fire within debounce must lose"
        );

        // Cleanup
        let _ = std::fs::remove_file(debounce_path(&sid));
    }

    #[test]
    fn try_reserve_fire_after_should_fire_is_consistent() {
        // The cheap pre-check and the atomic reserve must agree on a
        // fresh session: both say "fire."
        let sid = format!(
            "memex-test-reserve-consistency-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let _ = std::fs::remove_file(debounce_path(&sid));

        assert!(should_fire_for_session(&sid));
        assert!(try_reserve_fire(&sid));
        // After reserve: both refuse.
        assert!(!should_fire_for_session(&sid));
        assert!(!try_reserve_fire(&sid));

        let _ = std::fs::remove_file(debounce_path(&sid));
    }
}
