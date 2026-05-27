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
}
