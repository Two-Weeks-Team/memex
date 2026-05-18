//! Integration tests for the Codex JSONL parser. Run with
//! `cargo test --test codex_parser_integration`.
//!
//! Pairs with the unit tests in `src-tauri/src/codex_parser.rs::tests`. These
//! tests exercise the fixture files committed in
//! `src-tauri/tests/fixtures_codex/` so any schema-shape regression caught
//! at fixture load time fires here.

use std::path::PathBuf;

use memex_lib::codex_parser::{parse_codex_session, scan_codex_dir};
use memex_lib::parser::TurnRole;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures_codex")
        .join(name)
}

#[test]
fn t_parse_minimal_codex_one_turn() {
    let s = parse_codex_session(&fixture("rollout-01-minimal.jsonl")).unwrap();
    assert_eq!(s.event_counts.user, 1);
    assert_eq!(s.event_counts.assistant, 1);
    assert_eq!(s.turns.len(), 2);
    assert_eq!(s.session_id, "019e39e6-aaaa-bbbb-cccc-000000000001");
    // ai_title synthesized from the first user message.
    assert!(s.ai_title.is_some());
    let title = s.ai_title.as_deref().unwrap();
    assert!(title.starts_with("What does"));
}

#[test]
fn t_parse_codex_with_tools() {
    let s = parse_codex_session(&fixture("rollout-02-with-tools.jsonl")).unwrap();
    let tool_count: usize = s.turns.iter().map(|t| t.tool_calls.len()).sum();
    assert_eq!(tool_count, 2);
    let result_count: usize = s.turns.iter().map(|t| t.tool_results.len()).sum();
    assert_eq!(result_count, 2);
    // Confirm tool_call arguments string was parsed back to JSON values.
    let exec = s
        .turns
        .iter()
        .flat_map(|t| t.tool_calls.iter())
        .find(|tc| tc.name == "exec_command")
        .unwrap();
    assert_eq!(exec.input.get("cmd").and_then(|v| v.as_str()), Some("ls"));
}

#[test]
fn t_parse_codex_with_errors() {
    let s = parse_codex_session(&fixture("rollout-03-with-errors.jsonl")).unwrap();
    let any_error = s
        .turns
        .iter()
        .any(|t| t.tool_results.iter().any(|r| r.is_error));
    assert!(any_error, "fixture 03 must have at least one error result");
}

#[test]
fn t_parse_codex_long_session() {
    let s = parse_codex_session(&fixture("rollout-04-long-session.jsonl")).unwrap();
    assert_eq!(s.event_counts.user, 25);
    assert_eq!(s.event_counts.assistant, 25);
    let tool_count: usize = s.turns.iter().map(|t| t.tool_calls.len()).sum();
    // Every 5th iteration (i==4,9,14,19,24) adds a tool call → 5 total.
    assert_eq!(tool_count, 5);
}

#[test]
fn t_parse_codex_empty_after_meta() {
    let s = parse_codex_session(&fixture("rollout-05-empty-after-meta.jsonl")).unwrap();
    assert_eq!(s.event_counts.user, 0);
    assert_eq!(s.event_counts.assistant, 0);
    assert!(s.turns.is_empty());
    // ai_title remains None when there are no user messages.
    assert!(s.ai_title.is_none());
}

#[test]
fn t_scan_codex_dir_loads_all_fixtures() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures_codex");
    let mut sessions = scan_codex_dir(&dir).unwrap();
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    assert_eq!(sessions.len(), 5, "all 5 fixture files must parse");
    let ids: Vec<String> = sessions.iter().map(|s| s.session_id.clone()).collect();
    assert_eq!(
        ids[0],
        "019e39e6-aaaa-bbbb-cccc-000000000001",
        "first session_id mismatch"
    );
}

#[test]
fn t_codex_session_id_matches_meta() {
    let s = parse_codex_session(&fixture("rollout-02-with-tools.jsonl")).unwrap();
    // Verifies session_meta.payload.id is the source of session_id (NOT the
    // file name like Claude's parser).
    assert_eq!(s.session_id, "019e39e6-aaaa-bbbb-cccc-000000000002");
}

#[test]
fn t_codex_project_path_no_decoding() {
    let s = parse_codex_session(&fixture("rollout-02-with-tools.jsonl")).unwrap();
    // Codex stores cwd as already-absolute; the parser must take it verbatim.
    assert_eq!(
        s.project_path.as_deref(),
        Some("/Users/test/projects/demo")
    );
    assert_eq!(s.project_name.as_deref(), Some("demo"));
}

#[test]
fn t_codex_ai_title_derives_from_first_user() {
    let s = parse_codex_session(&fixture("rollout-01-minimal.jsonl")).unwrap();
    // The first user message text is "What does the parser do?"; ai_title
    // is the first 60 chars with newlines stripped.
    assert_eq!(
        s.ai_title.as_deref(),
        Some("What does the parser do?")
    );
}

#[test]
fn t_codex_function_call_attached_to_assistant() {
    let s = parse_codex_session(&fixture("rollout-02-with-tools.jsonl")).unwrap();
    // Both tool calls must live on an assistant turn (never on a user turn).
    for t in &s.turns {
        if !t.tool_calls.is_empty() {
            assert_eq!(
                t.role,
                TurnRole::Assistant,
                "tool_calls must live on assistant turns, got {:?}",
                t.role
            );
        }
    }
}

#[test]
fn t_codex_tool_result_paired_by_id() {
    let s = parse_codex_session(&fixture("rollout-02-with-tools.jsonl")).unwrap();
    // Build the set of call_ids and verify each one has a matching tool_result.
    let call_ids: Vec<String> = s
        .turns
        .iter()
        .flat_map(|t| t.tool_calls.iter())
        .map(|tc| tc.id.clone())
        .collect();
    let result_ids: Vec<String> = s
        .turns
        .iter()
        .flat_map(|t| t.tool_results.iter())
        .map(|r| r.tool_use_id.clone())
        .collect();
    for cid in &call_ids {
        assert!(result_ids.contains(cid), "call_id {cid} missing result");
    }
}
