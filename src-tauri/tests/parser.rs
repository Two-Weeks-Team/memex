//! Integration tests for the JSONL parser. Run with: `cargo test --test parser`.

use std::path::PathBuf;

use memex_lib::parser::{parse_session, scan_dir, TurnRole};

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
