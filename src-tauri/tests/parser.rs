//! Integration tests for the JSONL parser. Run with: `cargo test --test parser`.

use std::path::PathBuf;

use memex_lib::parser::{
    parse_session, parse_transcript_session, read_prompt_history_stats, scan_dir,
    scan_transcripts_dir, TurnRole,
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn parse_minimal_one_turn() {
    let s = parse_session(&fixture("01_minimal.jsonl")).unwrap();
    assert_eq!(s.session_id, "f1");
    assert_eq!(s.event_counts.user, 1);
    assert_eq!(s.event_counts.assistant, 1);
    assert_eq!(s.turns.len(), 2);
    assert_eq!(s.turns[0].text, "hello");
    assert_eq!(s.turns[1].text, "hi");
    assert!(s.turns[1].tool_calls.is_empty());
    assert_eq!(s.project_name.as_deref(), Some("proj-a"));
    assert_eq!(s.git_branch.as_deref(), Some("main"));
}

#[test]
fn parse_with_tool_use_and_result() {
    let s = parse_session(&fixture("02_with_tool_use.jsonl")).unwrap();
    assert_eq!(s.event_counts.user, 2);
    assert_eq!(s.event_counts.assistant, 1);

    let assistant = s
        .turns
        .iter()
        .find(|t| t.role == TurnRole::Assistant)
        .expect("assistant turn present");
    assert_eq!(assistant.tool_calls.len(), 1);
    assert_eq!(assistant.tool_calls[0].name, "Bash");
    assert_eq!(
        assistant.tool_calls[0].input["command"].as_str(),
        Some("ls")
    );

    let last_user = s
        .turns
        .iter()
        .rev()
        .find(|t| t.role == TurnRole::User)
        .expect("user turn present");
    assert_eq!(last_user.tool_results.len(), 1);
    assert_eq!(last_user.tool_results[0].content, "file1\nfile2");
    assert!(!last_user.tool_results[0].is_error);
}

#[test]
fn parse_records_ai_title_and_metadata() {
    let s = parse_session(&fixture("03_ai_title.jsonl")).unwrap();
    assert_eq!(s.ai_title.as_deref(), Some("Building Memex Day 1"));
    assert_eq!(s.project_name.as_deref(), Some("memex"));
    assert_eq!(s.git_branch.as_deref(), Some("main"));
    assert_eq!(s.claude_version.as_deref(), Some("2.1.0"));
    assert_eq!(s.event_counts.user, 1);
    assert_eq!(s.event_counts.assistant, 1);
    // ai-title + attachment = 2 "other" events
    assert_eq!(s.event_counts.other, 2);
}

#[test]
fn parse_marks_tool_errors() {
    let s = parse_session(&fixture("04_tool_error.jsonl")).unwrap();
    let last_user = s
        .turns
        .iter()
        .rev()
        .find(|t| t.role == TurnRole::User)
        .expect("user turn with tool_result");
    assert_eq!(last_user.tool_results.len(), 1);
    assert!(last_user.tool_results[0].is_error);
    assert!(last_user.tool_results[0].content.contains("command failed"));
}

#[test]
fn parse_mixed_handles_system_and_sidechain() {
    let s = parse_session(&fixture("05_mixed.jsonl")).unwrap();
    assert_eq!(s.event_counts.system, 1);

    let assistant = s
        .turns
        .iter()
        .find(|t| t.role == TurnRole::Assistant)
        .expect("assistant turn");
    assert_eq!(assistant.tool_calls.len(), 2);
    let names: Vec<&str> = assistant.tool_calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["Read", "Edit"]);

    let last_user = s
        .turns
        .iter()
        .rev()
        .find(|t| t.role == TurnRole::User)
        .expect("user turn");
    assert!(last_user.is_sidechain);
    assert_eq!(last_user.tool_results.len(), 2);
}

#[test]
fn parse_records_time_bounds() {
    let s = parse_session(&fixture("05_mixed.jsonl")).unwrap();
    let start = s.start_time.expect("start time");
    let end = s.end_time.expect("end time");
    assert!(start <= end, "start <= end");
    assert_eq!(start.to_rfc3339(), "2026-05-18T13:00:00+00:00");
    assert_eq!(end.to_rfc3339(), "2026-05-18T13:00:03+00:00");
}

#[test]
fn scan_dir_finds_all_fixtures() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut sessions = scan_dir(&dir).expect("scan should succeed");
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    assert_eq!(sessions.len(), 5);
    let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids, vec!["f1", "f2", "f3", "f4", "f5"]);
}

#[test]
fn scan_skips_subagents_dir() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures_with_subagent");
    let sessions = scan_dir(&dir).expect("scan should succeed");
    assert_eq!(
        sessions.len(),
        1,
        "subagents/ traces must be skipped at scan time"
    );
    assert_eq!(sessions[0].session_id, "sa1");
}

// ----- legacy `~/.claude/transcripts/` schema (rollout before v2.1.114) -----

fn transcripts_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures_transcripts")
        .join(name)
}

#[test]
fn parse_transcript_minimal_user_and_tool() {
    let s = parse_transcript_session(&transcripts_fixture("ses_minimal.jsonl")).unwrap();
    assert_eq!(s.session_id, "ses_minimal", "session_id from file stem");
    assert_eq!(s.project_name.as_deref(), Some("(legacy transcript)"));
    assert!(s.project_path.is_none());

    // user → assistant (tool_use + tool_result) → user
    assert_eq!(s.event_counts.user, 2);
    assert_eq!(s.turns.len(), 3, "two user turns + one assistant turn");

    assert_eq!(s.turns[0].role, TurnRole::User);
    assert_eq!(s.turns[0].text, "hello");

    assert_eq!(s.turns[1].role, TurnRole::Assistant);
    assert_eq!(s.turns[1].tool_calls.len(), 1);
    assert_eq!(s.turns[1].tool_calls[0].name, "Bash");
    assert_eq!(s.turns[1].tool_results.len(), 1);
    assert!(s.turns[1].tool_results[0].content.contains("file.txt"));
    assert!(!s.turns[1].tool_results[0].is_error, "no error keywords in output");

    assert_eq!(s.turns[2].role, TurnRole::User);
    assert_eq!(s.turns[2].text, "ok");

    let start = s.start_time.unwrap();
    let end = s.end_time.unwrap();
    assert_eq!(start.to_rfc3339(), "2026-02-12T13:56:58.762+00:00");
    assert_eq!(end.to_rfc3339(), "2026-02-12T13:57:05+00:00");
}

#[test]
fn parse_transcript_detects_error_heuristic() {
    let s = parse_transcript_session(&transcripts_fixture("ses_with_error.jsonl")).unwrap();
    assert_eq!(s.turns.len(), 2);
    let assistant = s
        .turns
        .iter()
        .find(|t| t.role == TurnRole::Assistant)
        .expect("assistant turn");
    assert_eq!(assistant.tool_results.len(), 1);
    assert!(
        assistant.tool_results[0].is_error,
        "tool_output containing 'Error:' / 'Traceback' should set is_error"
    );
}

#[test]
fn scan_transcripts_dir_picks_up_ses_files() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures_transcripts");
    let mut sessions = scan_transcripts_dir(&dir).expect("transcripts scan");
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].session_id, "ses_minimal");
    assert_eq!(sessions[1].session_id, "ses_with_error");
    for s in &sessions {
        assert_eq!(s.project_name.as_deref(), Some("(legacy transcript)"));
    }
}

#[test]
fn scan_transcripts_missing_root_returns_empty() {
    // Clean installs won't have ~/.claude/transcripts/ at all — must not error.
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/no-such-dir-zzzzz");
    let sessions = scan_transcripts_dir(&dir).expect("missing root should be Ok([])");
    assert!(sessions.is_empty());
}

// ----- ~/.claude/history.jsonl — the prompt timeline base layer -----

#[test]
fn prompt_history_aggregates_by_day_and_counts_projects() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures_history/history.jsonl");
    let stats = read_prompt_history_stats(&p).expect("should parse");

    // 6 valid lines + 2 malformed (skipped silently)
    assert_eq!(stats.total_prompts, 6, "malformed lines must be skipped, not error");

    // 3 distinct projects in the fixture
    assert_eq!(stats.project_count, 3);

    // Day buckets must be non-empty
    let total_in_buckets: usize = stats.by_day.values().sum();
    assert_eq!(total_in_buckets, stats.total_prompts);

    // Earliest/latest match the fixture's timestamps
    assert_eq!(stats.earliest_ms, Some(1_759_230_433_797));
    assert_eq!(stats.latest_ms, Some(1_759_489_600_000));
}

#[test]
fn prompt_history_missing_file_returns_empty_stats() {
    // A clean install has no ~/.claude/history.jsonl yet — must not error.
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures_history/does-not-exist.jsonl");
    let stats = read_prompt_history_stats(&p).expect("missing file is Ok(empty)");
    assert_eq!(stats.total_prompts, 0);
    assert!(stats.by_day.is_empty());
    assert_eq!(stats.earliest_ms, None);
    assert_eq!(stats.latest_ms, None);
    assert_eq!(stats.project_count, 0);
}
