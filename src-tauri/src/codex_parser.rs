//! Codex CLI session JSONL parser (KH-01).
//!
//! Codex stores rollouts under `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`.
//! The schema (verified against real sessions in
//! `~/.codex/sessions/2026/05/18/rollout-*.jsonl`) is:
//!
//! ```jsonc
//! // Line 1 — always exactly one session_meta:
//! {
//!   "timestamp": "<ISO 8601>",
//!   "type": "session_meta",
//!   "payload": {
//!     "id": "<uuid>",                    // → session.session_id
//!     "timestamp": "<ISO 8601>",
//!     "cwd": "/abs/path",                // → session.project_path (already absolute, NO decode)
//!     "originator": "Codex Desktop"|"Codex CLI"|...,
//!     "cli_version": "0.131.0-alpha.9",  // → session.claude_version (re-used field)
//!     "source": "vscode"|...,
//!     "model_provider": "openai",
//!     "base_instructions": { "text": "..." }
//!   }
//! }
//! // Subsequent lines:
//! { "timestamp": "...", "type": "event_msg",    "payload": { "type": "task_started"|..., "turn_id": ..., "started_at": ... } }
//! { "timestamp": "...", "type": "turn_context", "payload": { "turn_id": ..., "cwd": ..., ... } }
//! { "timestamp": "...", "type": "response_item", "payload": {
//!     "type": "message"|"reasoning"|"function_call"|"function_call_output",
//!     "role": "developer"|"user"|"assistant",    // for "message"
//!     "content": [{ "type": "input_text"|"output_text", "text": "..." }, ...]
//! }}
//! ```
//!
//! ## Mapping to the common `Session`/`Turn`/`ToolCall` types
//!
//! | Codex source                                  | Memex `Session`/`Turn` field                |
//! |-----------------------------------------------|---------------------------------------------|
//! | `session_meta.payload.id`                     | `session.session_id`                        |
//! | `session_meta.payload.cwd`                    | `session.project_path`, derive `project_name` |
//! | `session_meta.payload.cli_version`            | `session.claude_version` (re-purposed)      |
//! | top-level `timestamp` of session_meta         | `session.start_time`                        |
//! | top-level `timestamp` of last line            | `session.end_time`                          |
//! | `response_item.message` with `role=user`      | `Turn { role: User, text }`                 |
//! | `response_item.message` with `role=assistant` | `Turn { role: Assistant, text }`            |
//! | `response_item.function_call`                 | Most-recent assistant Turn gains a ToolCall |
//! | `response_item.function_call_output`          | Subsequent User Turn gains a ToolResult     |
//! | `has_errors`: presence of error markers in `function_call_output.output` | `session.has_errors` analog (computed via tool_results) |
//!
//! SPEC NOTE (P5, KH-01): Codex doesn't carry an `ai-title` event. We
//! synthesize `session.ai_title` from the first 60 chars of the first user
//! message text so the Time Machine row still has a label.
//!
//! SPEC NOTE (P5, KH-01): Codex doesn't carry `gitBranch`. We leave
//! `session.git_branch` as `None` and the v3 payload pipeline serializes that
//! as the empty string (matches existing Claude behavior for branchless paths).
//!
//! SPEC NOTE (P5, KH-01): the `cwd` field is already absolute in the Codex
//! schema. **Do NOT** apply the Claude-style "encoded-cwd → decoded path"
//! transform that `parser.rs::project_name_from_cwd` does — that lives in
//! the *filename* of Claude session directories, not in Codex payloads.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use walkdir::WalkDir;

use crate::parser::{EventCounts, Session, ToolCall, ToolResult, Turn, TurnRole};

const AI_TITLE_PREVIEW_CHARS: usize = 60;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a single Codex `rollout-*.jsonl` file into a `Session`.
///
/// Returns `Err` when:
/// - The file cannot be opened or read.
/// - The first content line is missing or not a valid `session_meta`.
/// - Any JSONL line is not valid JSON.
pub fn parse_codex_session(path: &Path) -> Result<Session> {
    let file = File::open(path)
        .with_context(|| format!("opening codex rollout: {}", path.display()))?;
    let reader = BufReader::new(file);

    let mut session: Option<Session> = None;
    let mut start_time: Option<DateTime<Utc>> = None;
    let mut end_time: Option<DateTime<Utc>> = None;
    // function_call_id -> (tool_name, input)  — populated from function_call
    // lines so we can pair the subsequent function_call_output with its
    // originator.
    let mut pending_calls: HashMap<String, (String, Value)> = HashMap::new();
    // session_meta seen flag — exactly one allowed; missing → Err.
    let mut meta_seen = false;
    // user_msg / assistant_msg counts.
    let mut counts = EventCounts::default();

    for (lineno, line_res) in reader.lines().enumerate() {
        let line = line_res
            .with_context(|| format!("reading line {} of {}", lineno + 1, path.display()))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let obj: Value = serde_json::from_str(line)
            .with_context(|| format!("parsing json on line {} of {}", lineno + 1, path.display()))?;

        let event_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
        let line_ts = obj
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        if let Some(t) = line_ts {
            start_time = Some(match start_time {
                Some(s) if s <= t => s,
                _ => t,
            });
            end_time = Some(match end_time {
                Some(e) if e >= t => e,
                _ => t,
            });
        }

        match event_type {
            "session_meta" => {
                if meta_seen {
                    // Tolerate (some Codex versions can emit two; the second
                    // is informational) — keep the first.
                    continue;
                }
                meta_seen = true;
                session = Some(session_from_meta(path, &obj)?);
            }
            "response_item" => {
                if session.is_none() {
                    bail!(
                        "{}: response_item on line {} before session_meta",
                        path.display(),
                        lineno + 1
                    );
                }
                let s = session.as_mut().unwrap();
                handle_response_item(s, &obj, line_ts, &mut pending_calls, &mut counts);
            }
            // event_msg, turn_context, etc. — not lifted into Turn/ToolCall.
            // We still let their timestamps influence start/end bounds (handled
            // above).
            _ => {
                counts.other = counts.other.saturating_add(1);
            }
        }
    }

    let mut session = session.ok_or_else(|| {
        anyhow!(
            "{}: no session_meta line found — not a Codex rollout file",
            path.display()
        )
    })?;
    session.start_time = start_time;
    session.end_time = end_time;
    session.event_counts = counts;

    // Synthesize ai_title from first user message if absent.
    if session.ai_title.is_none() {
        if let Some(t) = session
            .turns
            .iter()
            .find(|t| t.role == TurnRole::User && !t.text.trim().is_empty())
        {
            let preview: String = t.text.chars().take(AI_TITLE_PREVIEW_CHARS).collect();
            let cleaned = preview.replace('\n', " ").trim().to_string();
            if !cleaned.is_empty() {
                session.ai_title = Some(cleaned);
            }
        }
    }

    Ok(session)
}

/// Walk a Codex sessions root (typically `~/.codex/sessions`) and parse every
/// `rollout-*.jsonl` file. Other files (e.g. `.codex/sessions/.config`) are
/// ignored.
pub fn scan_codex_dir(root: &Path) -> Result<Vec<Session>> {
    if !root.exists() {
        return Err(anyhow!("codex scan root does not exist: {}", root.display()));
    }
    let mut sessions = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        if !is_rollout_file(path) {
            continue;
        }
        match parse_codex_session(path) {
            Ok(s) => sessions.push(s),
            Err(e) => errors.push(format!("{}: {:#}", path.display(), e)),
        }
    }

    // If we have *some* sessions, return them even if a few files failed —
    // matches `parser::scan_dir`'s tolerance.
    if sessions.is_empty() && !errors.is_empty() {
        return Err(anyhow!(
            "no codex sessions parsed; {} error(s); first: {}",
            errors.len(),
            errors[0]
        ));
    }
    Ok(sessions)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// `true` if `path` is a Codex rollout (extension == `jsonl`, filename starts
/// with `rollout-`).
fn is_rollout_file(path: &Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return false;
    }
    path.file_name()
        .and_then(|f| f.to_str())
        .map(|f| f.starts_with("rollout-"))
        .unwrap_or(false)
}

/// Build the initial Session skeleton from a `session_meta` event line.
fn session_from_meta(path: &Path, obj: &Value) -> Result<Session> {
    let payload = obj
        .get("payload")
        .ok_or_else(|| anyhow!("session_meta missing payload"))?;
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("session_meta.payload.id missing"))?
        .to_string();
    let cwd = payload.get("cwd").and_then(Value::as_str).map(str::to_string);
    let project_name = cwd
        .as_deref()
        .map(project_name_from_abs_cwd)
        .unwrap_or_default();
    let cli_version = payload
        .get("cli_version")
        .and_then(Value::as_str)
        .map(str::to_string);

    Ok(Session {
        session_id: id,
        source_path: path.to_path_buf(),
        project_path: cwd,
        project_name: if project_name.is_empty() {
            None
        } else {
            Some(project_name)
        },
        // Codex doesn't expose a git branch field — left None.
        git_branch: None,
        // We reuse `claude_version` to carry Codex's `cli_version` so the v3
        // payload pipeline (which stamps schema_version=3 and source_agent=codex)
        // still gets a non-empty version string for UI display.
        claude_version: cli_version,
        // ai_title is synthesized from the first user message later, post-parse.
        ai_title: None,
        start_time: None,
        end_time: None,
        turns: Vec::new(),
        event_counts: EventCounts::default(),
    })
}

/// Handle one `response_item` payload — append Turn or ToolCall or ToolResult.
fn handle_response_item(
    session: &mut Session,
    obj: &Value,
    line_ts: Option<DateTime<Utc>>,
    pending_calls: &mut HashMap<String, (String, Value)>,
    counts: &mut EventCounts,
) {
    let payload = match obj.get("payload") {
        Some(p) => p,
        None => return,
    };
    let inner_type = payload.get("type").and_then(Value::as_str).unwrap_or("");

    match inner_type {
        "message" => {
            let role_str = payload.get("role").and_then(Value::as_str).unwrap_or("");
            // Only user/assistant become Turns. "developer" role lines are
            // system instructions (the long preamble) — we don't lift them
            // into Turns because they'd dominate every content vector.
            let role = match role_str {
                "user" => TurnRole::User,
                "assistant" => TurnRole::Assistant,
                _ => return,
            };
            let text = extract_message_text(payload);
            if text.trim().is_empty() {
                // Empty user messages occasionally appear (e.g., a "/init"
                // ping). Skip them so they don't pollute counts.
                return;
            }
            match role {
                TurnRole::User => counts.user = counts.user.saturating_add(1),
                TurnRole::Assistant => counts.assistant = counts.assistant.saturating_add(1),
                _ => {}
            }
            session.turns.push(Turn {
                // No turn-uuid field in Codex; synthesize a stable-ish id from
                // count + role so Turn::uuid is unique within the session.
                uuid: format!("codex-{}-{}", role_str, session.turns.len()),
                parent_uuid: None,
                timestamp: line_ts,
                role,
                is_sidechain: false,
                text,
                tool_calls: Vec::new(),
                tool_results: Vec::new(),
            });
        }
        "function_call" => {
            // Codex tool calls map to ToolCall on the most-recent assistant Turn.
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let call_id = payload
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            // Codex carries `arguments` as a JSON-encoded string; try to parse it.
            let input = payload
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .unwrap_or(Value::Null);
            pending_calls.insert(call_id.clone(), (name.clone(), input.clone()));

            // Attach to the most recent assistant Turn — create a stub Turn
            // if none exists yet (rare but defensive).
            let assistant_idx = session
                .turns
                .iter()
                .rposition(|t| t.role == TurnRole::Assistant);
            let target_idx = match assistant_idx {
                Some(i) => i,
                None => {
                    counts.assistant = counts.assistant.saturating_add(1);
                    session.turns.push(Turn {
                        uuid: format!("codex-fc-stub-{}", session.turns.len()),
                        parent_uuid: None,
                        timestamp: line_ts,
                        role: TurnRole::Assistant,
                        is_sidechain: false,
                        text: String::new(),
                        tool_calls: Vec::new(),
                        tool_results: Vec::new(),
                    });
                    session.turns.len() - 1
                }
            };
            session.turns[target_idx].tool_calls.push(ToolCall {
                id: call_id,
                name,
                input,
            });
        }
        "function_call_output" => {
            // Codex tool results carry the output as a single string.
            let call_id = payload
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let content = payload
                .get("output")
                .map(value_to_text)
                .unwrap_or_default();
            let is_error = looks_like_error(&content);

            // Match Claude's structure: the tool_result lives on the User Turn
            // that immediately follows the assistant tool call. Codex doesn't
            // have a user turn between them, so we attach to the NEXT user
            // Turn we encounter — or synthesize a placeholder User Turn.
            //
            // Simpler robust choice: attach to a synthesized "user-like" Turn
            // immediately following the assistant Turn that owns the call.
            // This keeps the existing `parser::Turn` invariants (results on
            // user-role turns) without requiring lookahead.
            let assistant_idx = session
                .turns
                .iter()
                .rposition(|t| t.role == TurnRole::Assistant);
            let owns_call =
                assistant_idx.map(|i| session.turns[i].tool_calls.iter().any(|tc| tc.id == call_id));
            if owns_call == Some(true) {
                // Look for a User Turn after the assistant; create one if missing.
                let assistant_i = assistant_idx.unwrap();
                let user_idx_after = session
                    .turns
                    .iter()
                    .enumerate()
                    .skip(assistant_i + 1)
                    .find(|(_, t)| t.role == TurnRole::User)
                    .map(|(i, _)| i);
                let target_user_idx = match user_idx_after {
                    Some(i) => i,
                    None => {
                        // Synthesize a placeholder user turn that holds the result.
                        // event_counts.user stays UNINCREMENTED — this is not a
                        // real user message, just a structural container.
                        session.turns.push(Turn {
                            uuid: format!("codex-out-stub-{}", session.turns.len()),
                            parent_uuid: None,
                            timestamp: line_ts,
                            role: TurnRole::User,
                            is_sidechain: false,
                            text: String::new(),
                            tool_calls: Vec::new(),
                            tool_results: Vec::new(),
                        });
                        session.turns.len() - 1
                    }
                };
                session.turns[target_user_idx].tool_results.push(ToolResult {
                    tool_use_id: call_id.clone(),
                    content,
                    is_error,
                });
            }
            // Drop the pending entry; we won't need it again.
            pending_calls.remove(&call_id);
        }
        // "reasoning" / encrypted thinking — not lifted into Turns.
        _ => {}
    }
}

/// Concatenate every `text` field inside a `payload.content[]` array.
fn extract_message_text(payload: &Value) -> String {
    let arr = match payload.get("content").and_then(Value::as_array) {
        Some(a) => a,
        None => return String::new(),
    };
    let mut out = String::new();
    for item in arr {
        if let Some(t) = item.get("text").and_then(Value::as_str) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(t);
        }
    }
    out
}

/// Reduce a JSON Value to a flat string suitable for `ToolResult.content`.
fn value_to_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Heuristic: does this Codex function_call output look like an error?
fn looks_like_error(out: &str) -> bool {
    let lower = out.to_lowercase();
    if lower.contains("process exited with code 0") {
        return false;
    }
    lower.contains("exit code 1")
        || lower.contains("exit code 2")
        || lower.contains("\nerror:")
        || lower.starts_with("error:")
        || lower.contains("traceback (most recent call last")
        || lower.contains("stderr:")
}

/// Codex's `cwd` is already absolute. Just take the basename — DO NOT do the
/// Claude-style encoded-cwd decode (`-Users-x-foo` → `/Users/x/foo`); Codex
/// payloads carry the path already-decoded.
fn project_name_from_abs_cwd(cwd: &str) -> String {
    PathBuf::from(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cwd)
        .to_string()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_jsonl(td: &TempDir, name: &str, lines: &[&str]) -> PathBuf {
        let p = td.path().join(name);
        fs::write(&p, lines.join("\n")).unwrap();
        p
    }

    #[test]
    fn t_is_rollout_file_accepts_rollout_jsonl() {
        assert!(is_rollout_file(Path::new("/x/2026/05/18/rollout-abc.jsonl")));
        assert!(!is_rollout_file(Path::new("/x/.config")));
        assert!(!is_rollout_file(Path::new("/x/something.jsonl"))); // missing rollout- prefix
        assert!(!is_rollout_file(Path::new("/x/rollout-abc.json")));
    }

    #[test]
    fn t_project_name_from_abs_cwd() {
        assert_eq!(
            project_name_from_abs_cwd("/Users/k/Documents/GitHub/memex"),
            "memex"
        );
        assert_eq!(project_name_from_abs_cwd("/"), "/");
    }

    #[test]
    fn t_looks_like_error_detection() {
        assert!(!looks_like_error("Process exited with code 0\nok"));
        assert!(looks_like_error("stderr: command failed"));
        assert!(looks_like_error("Error: file not found"));
        assert!(!looks_like_error("everything fine"));
    }

    #[test]
    fn t_missing_session_meta_returns_err() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(
            &td,
            "rollout-x.jsonl",
            &[r#"{"timestamp":"2026-05-18T00:00:00Z","type":"event_msg","payload":{}}"#],
        );
        assert!(parse_codex_session(&p).is_err());
    }

    #[test]
    fn t_malformed_json_returns_err() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(&td, "rollout-x.jsonl", &["{not json"]);
        assert!(parse_codex_session(&p).is_err());
    }

    #[test]
    fn t_empty_after_meta_no_panic() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(
            &td,
            "rollout-empty.jsonl",
            &[r#"{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{"id":"u1","cwd":"/proj","cli_version":"0.1"}}"#],
        );
        let s = parse_codex_session(&p).unwrap();
        assert_eq!(s.session_id, "u1");
        assert!(s.turns.is_empty());
    }

    #[test]
    fn t_simple_user_assistant_pair() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(
            &td,
            "rollout-uap.jsonl",
            &[
                r#"{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{"id":"sid1","cwd":"/proj","cli_version":"0.1"}}"#,
                r#"{"timestamp":"2026-05-18T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#,
                r#"{"timestamp":"2026-05-18T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi back"}]}}"#,
            ],
        );
        let s = parse_codex_session(&p).unwrap();
        assert_eq!(s.event_counts.user, 1);
        assert_eq!(s.event_counts.assistant, 1);
        assert_eq!(s.turns.len(), 2);
        assert_eq!(s.turns[0].text, "hello");
        assert_eq!(s.turns[1].text, "hi back");
        // ai_title synthesized from first user message.
        assert_eq!(s.ai_title.as_deref(), Some("hello"));
    }

    #[test]
    fn t_function_call_attached_to_assistant() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(
            &td,
            "rollout-fc.jsonl",
            &[
                r#"{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{"id":"sid2","cwd":"/proj","cli_version":"0.1"}}"#,
                r#"{"timestamp":"2026-05-18T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"run a command"}]}}"#,
                r#"{"timestamp":"2026-05-18T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"sure"}]}}"#,
                r#"{"timestamp":"2026-05-18T00:00:03Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls\"}","call_id":"call-1"}}"#,
                r#"{"timestamp":"2026-05-18T00:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"a\nb\nProcess exited with code 0"}}"#,
            ],
        );
        let s = parse_codex_session(&p).unwrap();
        // tool_count via sum of tool_calls.
        let tool_count: usize = s.turns.iter().map(|t| t.tool_calls.len()).sum();
        assert_eq!(tool_count, 1);
        let assistant = s
            .turns
            .iter()
            .find(|t| t.role == TurnRole::Assistant)
            .unwrap();
        assert_eq!(assistant.tool_calls[0].name, "exec_command");
        // tool_result attached to a subsequent user-role turn (synthesized stub).
        let any_result = s
            .turns
            .iter()
            .flat_map(|t| t.tool_results.iter())
            .any(|r| r.tool_use_id == "call-1");
        assert!(any_result, "function_call_output should be paired");
    }

    #[test]
    fn t_function_call_output_error_flag() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(
            &td,
            "rollout-err.jsonl",
            &[
                r#"{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{"id":"sid3","cwd":"/proj","cli_version":"0.1"}}"#,
                r#"{"timestamp":"2026-05-18T00:00:01Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}"#,
                r#"{"timestamp":"2026-05-18T00:00:02Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{}","call_id":"c2"}}"#,
                r#"{"timestamp":"2026-05-18T00:00:03Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c2","output":"stderr: something failed"}}"#,
            ],
        );
        let s = parse_codex_session(&p).unwrap();
        let r = s
            .turns
            .iter()
            .flat_map(|t| t.tool_results.iter())
            .find(|r| r.tool_use_id == "c2")
            .unwrap();
        assert!(r.is_error);
    }

    #[test]
    fn t_developer_role_excluded_from_turns() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(
            &td,
            "rollout-dev.jsonl",
            &[
                r#"{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{"id":"sid4","cwd":"/proj","cli_version":"0.1"}}"#,
                r#"{"timestamp":"2026-05-18T00:00:01Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"long preamble"}]}}"#,
                r#"{"timestamp":"2026-05-18T00:00:02Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}}"#,
            ],
        );
        let s = parse_codex_session(&p).unwrap();
        assert_eq!(s.turns.len(), 1);
        assert_eq!(s.turns[0].role, TurnRole::User);
    }

    #[test]
    fn t_project_path_unchanged_no_decoding() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(
            &td,
            "rollout-cwd.jsonl",
            &[
                r#"{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{"id":"sidcwd","cwd":"/Users/k/Documents/GitHub/memex","cli_version":"0.1"}}"#,
            ],
        );
        let s = parse_codex_session(&p).unwrap();
        // cwd is taken verbatim — no encoded-path decode.
        assert_eq!(s.project_path.as_deref(), Some("/Users/k/Documents/GitHub/memex"));
        assert_eq!(s.project_name.as_deref(), Some("memex"));
    }

    #[test]
    fn t_time_bounds() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(
            &td,
            "rollout-t.jsonl",
            &[
                r#"{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{"id":"sidt","cwd":"/proj","cli_version":"0.1"}}"#,
                r#"{"timestamp":"2026-05-18T00:00:05Z","type":"event_msg","payload":{}}"#,
                r#"{"timestamp":"2026-05-18T00:00:10Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}"#,
            ],
        );
        let s = parse_codex_session(&p).unwrap();
        assert_eq!(s.start_time.unwrap().to_rfc3339(), "2026-05-18T00:00:00+00:00");
        assert_eq!(s.end_time.unwrap().to_rfc3339(), "2026-05-18T00:00:10+00:00");
    }

    #[test]
    fn t_scan_codex_dir_only_rollout_files() {
        let td = TempDir::new().unwrap();
        // One valid rollout + one stray non-rollout jsonl.
        let day = td.path().join("2026/05/18");
        fs::create_dir_all(&day).unwrap();
        fs::write(
            day.join("rollout-abc.jsonl"),
            r#"{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{"id":"s","cwd":"/p","cli_version":"0.1"}}"#,
        )
        .unwrap();
        fs::write(day.join("not-a-rollout.jsonl"), r#"{"foo":1}"#).unwrap();

        let sessions = scan_codex_dir(td.path()).unwrap();
        assert_eq!(sessions.len(), 1, "must skip non-rollout files");
        assert_eq!(sessions[0].session_id, "s");
    }

    #[test]
    fn t_scan_codex_dir_recursive() {
        let td = TempDir::new().unwrap();
        // Two rollouts in different YYYY/MM/DD subdirs.
        for (date, sid) in [("2026/05/18", "s1"), ("2026/05/19", "s2")] {
            let day = td.path().join(date);
            fs::create_dir_all(&day).unwrap();
            fs::write(
                day.join(format!("rollout-{sid}.jsonl")),
                format!(
                    r#"{{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{{"id":"{sid}","cwd":"/p","cli_version":"0.1"}}}}"#,
                ),
            )
            .unwrap();
        }
        let mut sessions = scan_codex_dir(td.path()).unwrap();
        sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "s1");
        assert_eq!(sessions[1].session_id, "s2");
    }

    #[test]
    fn t_arguments_string_parsed_as_json() {
        let td = TempDir::new().unwrap();
        let p = write_jsonl(
            &td,
            "rollout-args.jsonl",
            &[
                r#"{"timestamp":"2026-05-18T00:00:00Z","type":"session_meta","payload":{"id":"sa","cwd":"/p","cli_version":"0.1"}}"#,
                r#"{"timestamp":"2026-05-18T00:00:01Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}"#,
                r#"{"timestamp":"2026-05-18T00:00:02Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"ls\",\"workdir\":\"/x\"}","call_id":"c"}}"#,
            ],
        );
        let s = parse_codex_session(&p).unwrap();
        let tc = s
            .turns
            .iter()
            .flat_map(|t| t.tool_calls.iter())
            .next()
            .unwrap();
        assert_eq!(tc.input.get("cmd").and_then(|v| v.as_str()), Some("ls"));
        assert_eq!(tc.input.get("workdir").and_then(|v| v.as_str()), Some("/x"));
    }
}
