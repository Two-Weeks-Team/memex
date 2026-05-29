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
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use walkdir::WalkDir;

/// Max chars for a title synthesized from the first user message.
const SYNTH_TITLE_CHARS: usize = 60;

/// A user "turn" beginning with one of these tags isn't real user input — it's
/// harness/system noise injected into the conversation (background task
/// events, system reminders, local command echoes). Skip such turns when
/// picking a message to title the session from, otherwise the title becomes
/// e.g. "<task-notification> <task-id>…".
const NOISE_PREFIXES: &[&str] = &[
    "<task-notification",
    "<system-reminder",
    "<local-command-caveat",
    "<local-command-stdout",
    "<bash-input",
    "<bash-stdout",
    "<bash-stderr",
];

/// True if a user message is harness-injected noise rather than real input.
fn is_noise_turn(text: &str) -> bool {
    let t = text.trim_start();
    NOISE_PREFIXES.iter().any(|p| t.starts_with(p))
}

/// Build a fallback session title from a user message, for sessions where
/// Claude never wrote an `ai-title` record. Prefers a slash-command's
/// `<command-args>` (the actual instruction); otherwise strips ALL injected
/// wrapper tags (`<command-…>`, `<task-id>`, `<output-file>`, …), then
/// collapses whitespace and truncates to [`SYNTH_TITLE_CHARS`].
fn synthesize_title(text: &str) -> String {
    static ARGS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)<command-args>(.*?)</command-args>").unwrap());
    // Any XML-ish wrapper tag the harness injects (open or close form).
    static TAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"</?[a-z][a-z0-9-]*[^>]*>").unwrap());
    let base = match ARGS.captures(text).and_then(|c| c.get(1)) {
        Some(m) if !m.as_str().trim().is_empty() => m.as_str().to_string(),
        _ => TAGS.replace_all(text, " ").into_owned(),
    };
    base.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(SYNTH_TITLE_CHARS)
        .collect()
}

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
            // Claude Code writes this field as `aiTitle` (camelCase). The old
            // `title` lookup never matched, so every session fell back to
            // "(untitled)" even when a perfectly good title was on disk.
            // Accept the legacy `title` key too, just in case.
            if let Some(t) = obj
                .get("aiTitle")
                .or_else(|| obj.get("title"))
                .and_then(Value::as_str)
            {
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

    // Fallback: Claude doesn't write an `ai-title` record for every session
    // (short or still-in-progress ones often lack one), so synthesize a title
    // from the first *real* user message. Skip harness-injected noise turns
    // (task notifications, system reminders, command echoes) — otherwise the
    // title becomes "<task-notification> <task-id>…". If every user turn is
    // noise, fall back to the first non-empty one so we still beat "(untitled)".
    if session.ai_title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        let pick = session
            .turns
            .iter()
            .find(|t| {
                t.role == TurnRole::User && !t.text.trim().is_empty() && !is_noise_turn(&t.text)
            })
            .or_else(|| {
                session
                    .turns
                    .iter()
                    .find(|t| t.role == TurnRole::User && !t.text.trim().is_empty())
            });
        if let Some(turn) = pick {
            let title = synthesize_title(&turn.text);
            if !title.is_empty() {
                session.ai_title = Some(title);
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

/// Walk the legacy `~/.claude/transcripts/` directory — flat dir of
/// `ses_<token>.jsonl` files written by Claude Code prior to the silent
/// migration to `~/.claude/projects/` around v2.1.114 (April 2026).
/// Schema is minimal (type ∈ {user, tool_use, tool_result}, no sessionId
/// or cwd inside the file). Memex preserves these so the user's older
/// 2–4 months of corpus survives Anthropic's path/format change.
pub fn scan_transcripts_dir(root: &Path) -> Result<Vec<Session>> {
    if !root.exists() {
        // Not an error — many users won't have transcripts/ (clean installs).
        return Ok(Vec::new());
    }
    let mut sessions = Vec::new();
    for entry in WalkDir::new(root)
        .max_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let file_stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if !file_stem.starts_with("ses_") {
            // Defensive: only treat ses_* as transcripts; the dir might
            // contain other artifacts in older versions.
            continue;
        }
        if let Ok(s) = parse_transcript_session(path) {
            sessions.push(s);
        }
    }
    Ok(sessions)
}

/// Aggregate per-day prompt count from `~/.claude/history.jsonl`. Claude
/// Code records every user prompt here regardless of which session-storage
/// format is active; the file survives the `transcripts/` → `projects/`
/// migration intact, so it is the single source of truth for the user's
/// *full* timeline (often 8+ months back, even when session jsonls only
/// go 30 days back due to silent cleanup). Returned in YYYY-MM-DD keys so
/// the frontend can drop straight into the activity heatmap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptHistoryStats {
    /// First seen prompt timestamp (epoch ms). None if file is empty.
    pub earliest_ms: Option<i64>,
    pub latest_ms: Option<i64>,
    pub total_prompts: usize,
    /// Per-day buckets — date string (YYYY-MM-DD UTC) → count.
    pub by_day: std::collections::HashMap<String, usize>,
    /// Distinct projects encountered.
    pub project_count: usize,
}

/// Parse `~/.claude/history.jsonl` and return per-day prompt counts.
/// Tolerant: malformed lines are skipped silently so the file's incremental
/// append-only growth doesn't break the dashboard.
pub fn read_prompt_history_stats(path: &Path) -> Result<PromptHistoryStats> {
    use std::collections::HashMap;
    use std::collections::HashSet;

    let file = match File::open(path) {
        Ok(f) => f,
        // No history.jsonl yet (clean install) — return empty rather than error.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PromptHistoryStats {
                earliest_ms: None,
                latest_ms: None,
                total_prompts: 0,
                by_day: HashMap::new(),
                project_count: 0,
            });
        }
        Err(e) => {
            return Err(anyhow!("opening history.jsonl: {e}"));
        }
    };
    let reader = BufReader::new(file);

    let mut by_day: HashMap<String, usize> = HashMap::new();
    let mut projects: HashSet<String> = HashSet::new();
    let mut earliest: Option<i64> = None;
    let mut latest: Option<i64> = None;
    let mut total: usize = 0;

    for line in reader.lines().map_while(|r| r.ok()) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(obj) = serde_json::from_str::<Value>(line) else { continue };
        // timestamp is epoch ms (number or string), tolerate both
        let ts_ms: Option<i64> = obj
            .get("timestamp")
            .and_then(|v| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
        let Some(ts_ms) = ts_ms else { continue };
        if ts_ms <= 0 {
            continue;
        }
        total += 1;
        earliest = Some(earliest.map_or(ts_ms, |e| e.min(ts_ms)));
        latest = Some(latest.map_or(ts_ms, |e| e.max(ts_ms)));

        // Bucket by the user's *local* day so the heatmap aligns with how
        // they actually experienced the work (a Korean user working at
        // 02:00 local should see that activity on the local day, not on
        // the previous UTC day). Memex always runs on the user's machine
        // so chrono::Local matches what the frontend sees with
        // `Date.toLocaleDateString("en-CA")`.
        if let Some(dt_utc) = chrono::DateTime::<Utc>::from_timestamp(ts_ms / 1000, 0) {
            let dt_local = dt_utc.with_timezone(&chrono::Local);
            let key = dt_local.format("%Y-%m-%d").to_string();
            *by_day.entry(key).or_insert(0) += 1;
        }
        if let Some(p) = obj.get("project").and_then(Value::as_str) {
            if !p.is_empty() {
                projects.insert(p.to_string());
            }
        }
    }

    Ok(PromptHistoryStats {
        earliest_ms: earliest,
        latest_ms: latest,
        total_prompts: total,
        by_day,
        project_count: projects.len(),
    })
}

/// Parse a single legacy transcript `ses_*.jsonl` into the same `Session`
/// shape the rest of the indexer/UI expects. We collapse consecutive
/// `tool_use`/`tool_result` events into a single Assistant turn so the
/// Replay engine + tool-Pareto stats stay consistent with the newer schema.
pub fn parse_transcript_session(path: &Path) -> Result<Session> {
    let file = File::open(path)
        .with_context(|| format!("opening transcript jsonl: {}", path.display()))?;
    let reader = BufReader::new(file);

    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let mut session = Session {
        session_id: session_id.clone(),
        source_path: path.to_path_buf(),
        project_path: None,
        // Tag this corpus visibly so the UI can distinguish legacy from new.
        project_name: Some("(legacy transcript)".to_string()),
        git_branch: None,
        claude_version: None,
        ai_title: None,
        start_time: None,
        end_time: None,
        turns: Vec::new(),
        event_counts: EventCounts::default(),
    };

    let mut current_assistant: Option<Turn> = None;
    let mut turn_seq: usize = 0;

    for (lineno, line_res) in reader.lines().enumerate() {
        let line = line_res
            .with_context(|| format!("reading transcript line {} of {}", lineno + 1, path.display()))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let obj: Value = match serde_json::from_str(line) {
            Ok(o) => o,
            Err(_) => continue, // tolerant — skip malformed lines
        };

        let ts = obj
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        if let Some(dt) = ts {
            session.start_time = Some(match session.start_time {
                Some(s) if s <= dt => s,
                _ => dt,
            });
            session.end_time = Some(match session.end_time {
                Some(e) if e >= dt => e,
                _ => dt,
            });
        }

        let t = obj.get("type").and_then(Value::as_str).unwrap_or("");
        match t {
            "user" => {
                // Close any open assistant turn first so ordering is preserved.
                if let Some(at) = current_assistant.take() {
                    session.turns.push(at);
                }
                let content = obj
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                session.turns.push(Turn {
                    uuid: format!("{session_id}-u{turn_seq}"),
                    parent_uuid: None,
                    timestamp: ts,
                    role: TurnRole::User,
                    is_sidechain: false,
                    text: content,
                    tool_calls: Vec::new(),
                    tool_results: Vec::new(),
                });
                session.event_counts.user += 1;
                turn_seq += 1;
            }
            "tool_use" => {
                let name = obj
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let input = obj.get("tool_input").cloned().unwrap_or(Value::Null);
                let tc = ToolCall {
                    id: format!("{session_id}-tu{turn_seq}"),
                    name,
                    input,
                };
                let turn = current_assistant.get_or_insert_with(|| Turn {
                    uuid: format!("{session_id}-a{turn_seq}"),
                    parent_uuid: None,
                    timestamp: ts,
                    role: TurnRole::Assistant,
                    is_sidechain: false,
                    text: String::new(),
                    tool_calls: Vec::new(),
                    tool_results: Vec::new(),
                });
                turn.tool_calls.push(tc);
                session.event_counts.assistant += 1;
            }
            "tool_result" => {
                let output = obj
                    .get("tool_output")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                // Legacy transcripts don't carry is_error flags — heuristic.
                let lower = output.to_ascii_lowercase();
                let is_error = lower.contains("error:")
                    || lower.contains("traceback")
                    || lower.contains("panic")
                    || lower.contains("command failed");
                let tr = ToolResult {
                    tool_use_id: format!("{session_id}-tu{turn_seq}"),
                    content: output,
                    is_error,
                };
                let turn = current_assistant.get_or_insert_with(|| Turn {
                    uuid: format!("{session_id}-a{turn_seq}"),
                    parent_uuid: None,
                    timestamp: ts,
                    role: TurnRole::Assistant,
                    is_sidechain: false,
                    text: String::new(),
                    tool_calls: Vec::new(),
                    tool_results: Vec::new(),
                });
                turn.tool_results.push(tr);
                session.event_counts.other += 1;
            }
            _ => {
                session.event_counts.other += 1;
            }
        }
    }
    if let Some(at) = current_assistant.take() {
        session.turns.push(at);
    }

    Ok(session)
}

/// Quick per-session summary line for CLI output.
pub fn summary_line(s: &Session) -> String {
    let project = s.project_name.as_deref().unwrap_or("?");
    let start = s
        .start_time
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "----".into());
    let tools: usize = s.turns.iter().map(|t| t.tool_calls.len()).sum();
    // Use the char-safe `truncate` helper — byte slicing (`&t[..60]`) would
    // panic on a non-UTF-8-boundary, which is easy to hit now that titles can
    // be non-ASCII (e.g. Korean).
    let title = s
        .ai_title
        .as_deref()
        .map(|t| truncate(t, 60))
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

#[cfg(test)]
mod title_tests {
    use super::*;
    use std::io::Write;

    fn jsonl(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn ai_title_reads_camelcase_aititle_field() {
        // Regression: the parser used to read `title`; Claude writes `aiTitle`,
        // so every session fell back to "(untitled)".
        let f = jsonl(&[
            r#"{"type":"ai-title","aiTitle":"Plan SOTA v3.2 kickoff","sessionId":"s1"}"#,
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"hello"}}"#,
        ]);
        let s = parse_session(f.path()).unwrap();
        assert_eq!(s.ai_title.as_deref(), Some("Plan SOTA v3.2 kickoff"));
    }

    #[test]
    fn fallback_title_from_first_user_message_when_no_ai_title() {
        let f = jsonl(&[
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"fix the auth bug in login.rs"}}"#,
        ]);
        let s = parse_session(f.path()).unwrap();
        assert_eq!(s.ai_title.as_deref(), Some("fix the auth bug in login.rs"));
    }

    #[test]
    fn fallback_skips_task_notification_noise_turns() {
        // Regression for the "<task-notification> <task-id>…" title: the first
        // user turn is harness noise, so the title must come from the next
        // real user message.
        let f = jsonl(&[
            r#"{"type":"user","uuid":"u1","message":{"role":"user","content":"<task-notification> <task-id>bkm0e3i1e</task-id> <output-file>/tmp/x</output-file>"}}"#,
            r#"{"type":"user","uuid":"u2","message":{"role":"user","content":"add a loading spinner to the session list"}}"#,
        ]);
        let s = parse_session(f.path()).unwrap();
        assert_eq!(
            s.ai_title.as_deref(),
            Some("add a loading spinner to the session list")
        );
    }

    #[test]
    fn synthesize_title_strips_all_wrapper_tags() {
        let t = "<task-notification> <task-id>abc</task-id> hello world";
        assert_eq!(synthesize_title(t), "abc hello world");
    }

    #[test]
    fn synthesize_title_prefers_command_args() {
        let t = "<command-message>handon</command-message>\n<command-name>/handon</command-name>\n<command-args>load the latest handoff and continue</command-args>";
        assert_eq!(synthesize_title(t), "load the latest handoff and continue");
    }

    #[test]
    fn synthesize_title_strips_wrappers_when_no_args() {
        assert_eq!(synthesize_title("<command-name>/clear</command-name>"), "/clear");
    }

    #[test]
    fn synthesize_title_truncates_to_60_chars() {
        let long = "word ".repeat(100);
        assert!(synthesize_title(&long).chars().count() <= SYNTH_TITLE_CHARS);
    }
}
