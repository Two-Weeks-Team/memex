//! Claude Code session JSONL parser.
//!
//! Each `~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl` is a stream of
//! events keyed by a `type` field. We care about `user` and `assistant` turns
//! (each holding a `message` with role + content). Other event types
//! (`attachment`, `permission-mode`, `ai-title`, etc.) are recorded only as
//! session-level metadata counters.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

/// One parsed session = one top-level `.jsonl` file (subagent traces excluded).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub source_path: PathBuf,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub git_branch: Option<String>,
    pub claude_version: Option<String>,
    pub ai_title: Option<String>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub turns: Vec<Turn>,
    pub event_counts: EventCounts,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventCounts {
    pub user: usize,
    pub assistant: usize,
    pub system: usize,
    pub other: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    pub role: TurnRole,
    pub is_sidechain: bool,
    /// Concatenated text content (excluding tool_use / tool_result blobs).
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
    pub tool_results: Vec<ToolResult>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TurnRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub tool_use_id: String,
    /// `text` content of the tool_result (may be truncated/encoded).
    pub content: String,
    pub is_error: bool,
}

/// Parse a single `<session-uuid>.jsonl` file into a `Session`.
pub fn parse_session(path: &Path) -> Result<Session> {
    let file = File::open(path)
        .with_context(|| format!("opening session jsonl: {}", path.display()))?;
    let reader = BufReader::new(file);

    let session_id_fallback = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut session = Session {
        session_id: session_id_fallback,
        source_path: path.to_path_buf(),
        project_path: None,
        project_name: None,
        git_branch: None,
        claude_version: None,
        ai_title: None,
        start_time: None,
        end_time: None,
        turns: Vec::new(),
        event_counts: EventCounts::default(),
    };

    let mut session_id_seen = false;

    for (lineno, line_res) in reader.lines().enumerate() {
        let line = line_res
            .with_context(|| format!("reading line {} of {}", lineno + 1, path.display()))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let obj: Value = serde_json::from_str(line).with_context(|| {
            format!("parsing json on line {} of {}", lineno + 1, path.display())
        })?;

        // Record session id from any event that carries one.
        if let Some(sid) = obj.get("sessionId").and_then(Value::as_str) {
            if !session_id_seen {
                session.session_id = sid.to_string();
                session_id_seen = true;
            }
        }

        // Track project path / git branch / claude version once.
        if session.project_path.is_none() {
            if let Some(cwd) = obj.get("cwd").and_then(Value::as_str) {
                session.project_path = Some(cwd.to_string());
                session.project_name = Some(project_name_from_cwd(cwd));
            }
        }
        if session.git_branch.is_none() {
            if let Some(b) = obj.get("gitBranch").and_then(Value::as_str) {
                session.git_branch = Some(b.to_string());
            }
        }
        if session.claude_version.is_none() {
            if let Some(v) = obj.get("version").and_then(Value::as_str) {
                session.claude_version = Some(v.to_string());
            }
        }

        let event_type = obj.get("type").and_then(Value::as_str).unwrap_or("");

        if event_type == "ai-title" {
            if let Some(t) = obj.get("title").and_then(Value::as_str) {
                if session.ai_title.is_none() {
                    session.ai_title = Some(t.to_string());
                }
            }
        }

        // Update time bounds from any event that has a timestamp.
        if let Some(ts) = obj.get("timestamp").and_then(Value::as_str) {
            if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
                let dt_utc = dt.with_timezone(&Utc);
                session.start_time = Some(match session.start_time {
                    Some(s) if s <= dt_utc => s,
                    _ => dt_utc,
                });
                session.end_time = Some(match session.end_time {
                    Some(e) if e >= dt_utc => e,
                    _ => dt_utc,
                });
            }
        }

        match event_type {
            "user" => {
                session.event_counts.user += 1;
                if let Some(turn) = parse_turn(&obj, TurnRole::User) {
                    session.turns.push(turn);
                }
            }
            "assistant" => {
                session.event_counts.assistant += 1;
                if let Some(turn) = parse_turn(&obj, TurnRole::Assistant) {
                    session.turns.push(turn);
                }
            }
            "system" => {
                session.event_counts.system += 1;
            }
            _ => {
                session.event_counts.other += 1;
            }
        }
    }

    Ok(session)
}

fn parse_turn(obj: &Value, role: TurnRole) -> Option<Turn> {
    let uuid = obj.get("uuid").and_then(Value::as_str)?.to_string();
    let parent_uuid = obj
        .get("parentUuid")
        .and_then(Value::as_str)
        .map(str::to_string);
    let timestamp = obj
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc));
    let is_sidechain = obj
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut tool_results = Vec::new();

    if let Some(message) = obj.get("message") {
        if let Some(content) = message.get("content") {
            extract_content(content, &mut text, &mut tool_calls, &mut tool_results);
        }
    }

    Some(Turn {
        uuid,
        parent_uuid,
        timestamp,
        role,
        is_sidechain,
        text: text.trim().to_string(),
        tool_calls,
        tool_results,
    })
}

fn extract_content(
    content: &Value,
    text_out: &mut String,
    calls_out: &mut Vec<ToolCall>,
    results_out: &mut Vec<ToolResult>,
) {
    match content {
        Value::String(s) => {
            if !text_out.is_empty() {
                text_out.push('\n');
            }
            text_out.push_str(s);
        }
        Value::Array(items) => {
            for item in items {
                let Some(item_type) = item.get("type").and_then(Value::as_str) else {
                    continue;
                };
                match item_type {
                    "text" => {
                        if let Some(t) = item.get("text").and_then(Value::as_str) {
                            if !text_out.is_empty() {
                                text_out.push('\n');
                            }
                            text_out.push_str(t);
                        }
                    }
                    "tool_use" => {
                        let id = item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let name = item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let input = item.get("input").cloned().unwrap_or(Value::Null);
                        calls_out.push(ToolCall { id, name, input });
                    }
                    "tool_result" => {
                        let tool_use_id = item
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let is_error = item
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        let content_str = match item.get("content") {
                            Some(Value::String(s)) => s.clone(),
                            Some(Value::Array(parts)) => {
                                let mut buf = String::new();
                                for p in parts {
                                    if let Some(t) = p.get("text").and_then(Value::as_str) {
                                        if !buf.is_empty() {
                                            buf.push('\n');
                                        }
                                        buf.push_str(t);
                                    }
                                }
                                buf
                            }
                            _ => String::new(),
                        };
                        results_out.push(ToolResult {
                            tool_use_id,
                            content: content_str,
                            is_error,
                        });
                    }
                    _ => {
                        // image, thinking, etc. — ignore for now
                    }
                }
            }
        }
        _ => {}
    }
}

/// Derive a human-friendly project name from the absolute cwd path.
fn project_name_from_cwd(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(cwd)
        .to_string()
}

/// Walk a `~/.claude/projects`-style root and parse every top-level session
/// `.jsonl`. Subagent files under `*/subagents/` are skipped — they're tied to
/// their parent session and indexed separately if needed.
pub fn scan_dir(root: &Path) -> Result<Vec<Session>> {
    if !root.exists() {
        return Err(anyhow!("scan root does not exist: {}", root.display()));
    }
    let mut sessions = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        // Skip subagent traces (kept for replay-engine use later).
        if path
            .components()
            .any(|c| c.as_os_str() == "subagents")
        {
            continue;
        }
        match parse_session(path) {
            Ok(s) => sessions.push(s),
            Err(e) => errors.push(format!("{}: {:#}", path.display(), e)),
        }
    }

    if sessions.is_empty() && !errors.is_empty() {
        return Err(anyhow!(
            "no sessions parsed; {} error(s); first: {}",
            errors.len(),
            errors[0]
        ));
    }

    Ok(sessions)
}

/// Quick per-session summary line for CLI output.
pub fn summary_line(s: &Session) -> String {
    let project = s.project_name.as_deref().unwrap_or("?");
    let start = s
        .start_time
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "----".into());
    let tools: usize = s.turns.iter().map(|t| t.tool_calls.len()).sum();
    let title = s
        .ai_title
        .as_deref()
        .map(|t| {
            if t.len() > 60 {
                format!("{}…", &t[..60])
            } else {
                t.to_string()
            }
        })
        .unwrap_or_else(|| "(untitled)".into());
    format!(
        "{:<19} {:<24} u={:<3} a={:<3} tools={:<3} branch={:<10} {}",
        start,
        truncate(project, 24),
        s.event_counts.user,
        s.event_counts.assistant,
        tools,
        s.git_branch.as_deref().unwrap_or("-"),
        title
    )
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}
