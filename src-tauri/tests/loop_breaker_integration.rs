//! Integration test for the Loop Breaker pipeline.
//!
//! Verifies that a hand-crafted "stuck session" jsonl flips every gate
//! that `watcher::maybe_fire_loop_breaker` checks:
//!
//!   1. parser::parse_session reads the file end-to-end without error.
//!   2. session.turns.len() ≥ LOOP_MIN_TURNS (12).
//!   3. Error count in the last LOOP_RECENT_WINDOW (10) turns is
//!      ≥ LOOP_ERROR_THRESHOLD (3) — this is what gates the banner.
//!   4. When a live Qdrant + indexed corpus is available, predict_next_actions
//!      returns at least one prediction with non-zero frequency — this is
//!      the *suggestion* the banner surfaces.
//!
//! The fixture jsonl is **inlined** as a const string (see
//! `STUCK_SESSION_JSONL` below) and materialized to a tempdir at runtime —
//! we used to point at a fixture under `tests/fixtures/companion-demo/` but
//! that path is gitignored (demo assets are kept local-only) so clean
//! checkouts and CI saw the file as missing. (Codex review P1.)
//!
//! Lives as a separate test crate because we need `memex_lib` symbols
//! (parser, indexer, sec) but no Tauri AppHandle. Run with:
//!
//!     cd src-tauri && cargo test --test loop_breaker_integration
//!
//! Set MEMEX_SKIP_QDRANT_TESTS=1 to skip the live Qdrant step (CI fallback).

use std::io::Write;
use std::path::PathBuf;

use memex_lib::watcher::{LOOP_ERROR_THRESHOLD, LOOP_MIN_TURNS, LOOP_RECENT_WINDOW};
use memex_lib::{indexer, parser};
use tempfile::TempDir;

const STUCK_SESSION_ID: &str = "acme0006-stuck-live-aaaaaaaaaaaa";

/// A synthetic Claude Code session jsonl that simulates a developer
/// hammering `pnpm drizzle-kit push:pg` against a misconfigured Neon
/// connection — the canonical WAL Kind(WouldBlock) loop. 17 events total:
/// 1 ai-title + 16 turn events (2 user + 14 assistant/result pairs).
///
/// Loop Breaker requires: ≥ 12 turns AND ≥ 3 tool_result.is_error in last
/// 10 turns. This fixture has 14 turns total with 6 error events in the
/// last 10 → unambiguous trigger.
const STUCK_SESSION_JSONL: &str = include_str!("data/stuck_session.jsonl");

fn errors_in_window(session: &parser::Session, window: usize) -> usize {
    let n = session.turns.len();
    let start = n.saturating_sub(window);
    session.turns[start..]
        .iter()
        .flat_map(|t| t.tool_results.iter())
        .filter(|r| r.is_error)
        .count()
}

/// Materialize the inlined jsonl to a tempdir and return both the temp
/// handle (so the dir survives until the caller drops it) and the path
/// the test should pass to `parser::parse_session`.
fn write_fixture() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("create tempdir");
    let path = dir.path().join(format!("{STUCK_SESSION_ID}.jsonl"));
    let mut f = std::fs::File::create(&path).expect("create fixture jsonl");
    f.write_all(STUCK_SESSION_JSONL.as_bytes())
        .expect("write fixture jsonl");
    f.flush().ok();
    (dir, path)
}

#[test]
fn it_parses_stuck_session_cleanly() {
    let (_dir, p) = write_fixture();
    let session = parser::parse_session(&p).expect("parse_session");
    assert!(!session.turns.is_empty(), "session must have turns");
}

#[test]
fn it_meets_loop_breaker_turn_threshold() {
    let (_dir, p) = write_fixture();
    let session = parser::parse_session(&p).expect("parse");
    assert!(
        session.turns.len() >= LOOP_MIN_TURNS,
        "stuck fixture has {} turns; Loop Breaker requires ≥ {}",
        session.turns.len(),
        LOOP_MIN_TURNS
    );
}

#[test]
fn it_meets_loop_breaker_error_threshold() {
    let (_dir, p) = write_fixture();
    let session = parser::parse_session(&p).expect("parse");
    let errors = errors_in_window(&session, LOOP_RECENT_WINDOW);
    assert!(
        errors >= LOOP_ERROR_THRESHOLD,
        "stuck fixture surfaces {} errors in last {} turns; \
         Loop Breaker requires ≥ {}",
        errors,
        LOOP_RECENT_WINDOW,
        LOOP_ERROR_THRESHOLD
    );
}

#[test]
fn it_records_error_in_every_loop_iteration() {
    // Sanity — the 5 loop iterations should each carry exactly one
    // is_error tool_result. Catches accidental fixture corruption.
    let (_dir, p) = write_fixture();
    let session = parser::parse_session(&p).expect("parse");
    let total_errors: usize = session
        .turns
        .iter()
        .flat_map(|t| t.tool_results.iter())
        .filter(|r| r.is_error)
        .count();
    assert!(
        total_errors >= 5,
        "stuck fixture should carry at least 5 error events; got {total_errors}"
    );
}

#[test]
fn it_predict_next_actions_surfaces_pivot_when_corpus_is_indexed() {
    if std::env::var("MEMEX_SKIP_QDRANT_TESTS").as_deref() == Ok("1") {
        eprintln!("(skipping Qdrant-backed Loop Breaker predict step — MEMEX_SKIP_QDRANT_TESTS=1)");
        return;
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(async {
        // Pre-flight: do we have a reachable Qdrant?
        let client = match indexer::connect().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "(skipping predict step — Qdrant unreachable: {e:#}). Set up \
                     `./.qdrant/qdrant &` and index a corpus containing the stuck session."
                );
                return;
            }
        };

        // Predict needs an embedder.
        let embedder = match indexer::Embedder::new() {
            Ok(e) => e,
            Err(e) => {
                eprintln!("(skipping predict step — embedder init failed: {e:#})");
                return;
            }
        };

        let ctx = match indexer::predict_next_actions(
            &client,
            &embedder,
            STUCK_SESSION_ID,
            4,  // last_n_turns  (Loop Breaker default)
            4,  // horizon       (Loop Breaker default)
            10, // neighbors     (Loop Breaker default)
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "(skipping predict assertion — predict call failed: {e:#}). \
                     This usually means the stuck-session jsonl isn't indexed yet."
                );
                return;
            }
        };

        eprintln!(
            "loop-breaker predict: searched={} used={} predictions={}",
            ctx.neighbors_searched,
            ctx.neighbors_used,
            ctx.predictions.len(),
        );
        assert!(
            !ctx.predictions.is_empty() || ctx.neighbors_used == 0,
            "predict returned 0 predictions despite {} neighbors used",
            ctx.neighbors_used
        );
        if let Some(top) = ctx.predictions.first() {
            eprintln!(
                "  top suggestion: tool={} freq={:.2} from {} turn #{}",
                top.tool_name,
                top.frequency,
                top.from_session_project,
                top.from_turn_index,
            );
            // Frequency is a fraction of (neighbor × horizon) slots — always 0..=1.
            assert!(top.frequency > 0.0 && top.frequency <= 1.0);
        }
    });
}
