//! Heuristic, deterministic, LLM-free session enrichment (P5 Cat D).
//!
//! Populates the 5 enrich-stage payload fields reserved by P3
//! (`intent`, `entities`, `outcome`, `arc`, `topic`). Every function in this
//! module is a pure function of `(&Session, &[Turn])` — no I/O, no clocks,
//! no random sources, no HashMap iteration order dependence.
//!
//! Determinism guarantee (validated by `t_enrich_determinism`): calling
//! `enrich(s, &s.turns)` twice on the same input must return byte-identical
//! `EnrichmentOutput`. The hackathon pitch hard-bars any LLM call at runtime;
//! this module is the entire "AI summary" layer.
//!
//! ## Field semantics (heuristic spec — locked at P5 KICK time)
//!
//! - **intent**: majority tool-class.
//!   - Bash > 40% of total tool calls → `"build"`
//!   - Edit/Write > 40% → `"impl"`
//!   - Read > 40% → `"debug"`
//!   - none dominant → `"mixed"` (also when there are zero tool calls)
//! - **entities**: union of file-path tokens (regex on tool inputs/text) and
//!   first-word command names from Bash inputs. Top 10 by frequency, tiebroken
//!   by lexical order for determinism. Empty `Vec` if none found.
//! - **outcome**: scan the LAST assistant turn's text for resolved/unresolved
//!   markers (priority: resolved > unresolved > "partial"). Override to
//!   `"resolved"` if the *last* Bash tool call's tool_result reports exit_code=0.
//! - **arc**: tool-name sequence pattern over turns.
//!   - Read+ → Edit → Bash with no failure → `"fix"`
//!   - Read+ → Bash(failure markers) → Edit → `"debug-fix"`
//!   - Edit-mostly, no Bash → `"impl"`
//!   - Read-mostly, no Edit/Write → `"explore"`
//!   - otherwise → `"mixed"`
//! - **topic**: if `session.ai_title` is non-empty, return it verbatim.
//!   Otherwise `"<project_name> · <first_noun_phrase>"` from the first user
//!   message text (first 3-5 words, code-fences stripped, lowercased).
//!
//! See `tests/enrich_integration.rs` for spec-locked golden outputs.

use std::collections::BTreeMap;

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::parser::{Session, Turn, TurnRole};

/// Output of one enrichment pass over a Session.
///
/// All five fields use `String` (not `&str`) so the result is owned and can
/// flow directly into `V3Payload` / `serde_json::Value` without lifetime
/// concerns at the IPC boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichmentOutput {
    pub intent: String,
    pub entities: Vec<String>,
    pub outcome: String,
    pub arc: String,
    pub topic: String,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Compute the 5-field enrichment for a session. **Pure** — same input always
/// yields the same output. Safe to call from `index_session` per session.
pub fn enrich(session: &Session, turns: &[Turn]) -> EnrichmentOutput {
    EnrichmentOutput {
        intent: classify_intent(turns),
        entities: extract_entities(turns),
        outcome: classify_outcome(turns),
        arc: classify_arc(turns),
        topic: derive_topic(session, turns),
    }
}

// ---------------------------------------------------------------------------
// Intent
// ---------------------------------------------------------------------------

/// Threshold above which one tool family is declared dominant.
const INTENT_DOMINANCE: f32 = 0.40;

/// Determine the dominant tool family. Returns one of:
/// `"build"` (Bash dominant) | `"impl"` (Edit/Write dominant) | `"debug"`
/// (Read dominant) | `"mixed"` (no dominant family or zero tool calls).
pub fn classify_intent(turns: &[Turn]) -> String {
    let mut bash = 0usize;
    let mut edit = 0usize;
    let mut read = 0usize;
    let mut total = 0usize;
    for t in turns {
        for tc in &t.tool_calls {
            total += 1;
            match tc.name.as_str() {
                "Bash" => bash += 1,
                "Edit" | "MultiEdit" | "Write" => edit += 1,
                "Read" => read += 1,
                _ => {}
            }
        }
    }
    if total == 0 {
        return "mixed".to_string();
    }
    let f = |n: usize| n as f32 / total as f32;
    // Order matters when ties happen — Bash wins over Edit wins over Read
    // because builds/runs are the most "definitive" classification signal.
    if f(bash) > INTENT_DOMINANCE {
        "build".to_string()
    } else if f(edit) > INTENT_DOMINANCE {
        "impl".to_string()
    } else if f(read) > INTENT_DOMINANCE {
        "debug".to_string()
    } else {
        "mixed".to_string()
    }
}

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------

/// Matches file path-ish tokens: word chars + `-./` ending in a 1-5 char ext.
static FILE_PATH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([\w\-./]+\.\w{1,5})").unwrap());

const ENTITIES_TOP_N: usize = 10;

/// Extract the top-N file paths + Bash command names mentioned across turns.
/// Sorted by descending frequency, ties broken by lexical order (so two runs
/// over the same session produce byte-identical output — required by the
/// `t_enrich_determinism` invariant).
pub fn extract_entities(turns: &[Turn]) -> Vec<String> {
    // BTreeMap so iteration order is stable (lexical on the key); we then
    // re-sort by (-count, key) at the end.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();

    for t in turns {
        // Tool calls: paths from `file_path`, `path`, `pattern`, `command` etc.
        for tc in &t.tool_calls {
            // Per-tool path-ish fields.
            for key in ["file_path", "path", "notebook_path", "pattern"] {
                if let Some(v) = tc.input.get(key).and_then(|v| v.as_str()) {
                    if let Some(p) = normalize_path_token(v) {
                        *counts.entry(p).or_insert(0) += 1;
                    }
                }
            }
            // Bash: first word is the command name (e.g., "cargo", "git").
            if tc.name == "Bash" {
                if let Some(cmd) = tc.input.get("command").and_then(|v| v.as_str()) {
                    if let Some(first) = first_command_word(cmd) {
                        *counts.entry(first).or_insert(0) += 1;
                    }
                    // Also regex-scan the full command for embedded paths.
                    for cap in FILE_PATH_RE.captures_iter(cmd) {
                        if let Some(m) = cap.get(1) {
                            if let Some(p) = normalize_path_token(m.as_str()) {
                                *counts.entry(p).or_insert(0) += 1;
                            }
                        }
                    }
                }
            }
        }
        // Free text: regex-scan for path-ish tokens.
        for cap in FILE_PATH_RE.captures_iter(&t.text) {
            if let Some(m) = cap.get(1) {
                if let Some(p) = normalize_path_token(m.as_str()) {
                    *counts.entry(p).or_insert(0) += 1;
                }
            }
        }
    }

    // Sort by (-count, key) → top N. The BTreeMap iteration is already
    // deterministic on `key`, so the secondary order is also fixed.
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(ENTITIES_TOP_N)
        .map(|(k, _)| k)
        .collect()
}

/// Drop trailing punctuation, reject 1-2 char tokens (too noisy: ".rs", "py"),
/// and accept only tokens that include a `.` or `/` (file-like) OR are short
/// alpha command names that aren't trivially common words.
fn normalize_path_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim_end_matches(|c: char| matches!(c, '.' | ',' | ')' | ']' | ':' | ';'));
    if trimmed.len() < 3 {
        return None;
    }
    // Must contain `.` or `/` to be a path-ish token in this branch.
    if !(trimmed.contains('.') || trimmed.contains('/')) {
        return None;
    }
    Some(trimmed.to_string())
}

/// First word of a Bash command string (e.g., `cargo build` → `cargo`).
fn first_command_word(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim_start();
    let first = trimmed.split_whitespace().next()?;
    // Strip leading sudo/env prefixes (common in real sessions).
    let candidate = match first {
        "sudo" | "env" | "time" => trimmed.split_whitespace().nth(1)?,
        other => other,
    };
    // Reject path-like first-words; those are handled by the path-regex pass.
    if candidate.contains('/') || candidate.contains('\\') {
        return None;
    }
    if candidate.len() < 2 {
        return None;
    }
    Some(candidate.to_string())
}

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

const RESOLVED_TOKENS: &[&str] = &["resolved", "fixed", "works", "done", "success", "complete"];
const UNRESOLVED_TOKENS: &[&str] = &["fail", "error", "stuck", "broken", "cannot", "unable"];

/// Determine session outcome from the last assistant turn's text. Override to
/// `"resolved"` if the last Bash tool call's tool_result indicates exit_code=0.
pub fn classify_outcome(turns: &[Turn]) -> String {
    let last_assistant = turns.iter().rev().find(|t| t.role == TurnRole::Assistant);
    let mut outcome = "partial".to_string();
    if let Some(t) = last_assistant {
        let lower = t.text.to_lowercase();
        let has_resolved = RESOLVED_TOKENS.iter().any(|w| lower.contains(w));
        let has_unresolved = UNRESOLVED_TOKENS.iter().any(|w| lower.contains(w));
        if has_resolved && !has_unresolved {
            outcome = "resolved".to_string();
        } else if has_unresolved && !has_resolved {
            outcome = "unresolved".to_string();
        } else if has_resolved && has_unresolved {
            // Both present — defer to whichever last-mentioned wins.
            // Use the higher rfind index as the "more recent" sentiment.
            let res_idx = RESOLVED_TOKENS
                .iter()
                .filter_map(|w| lower.rfind(w))
                .max();
            let unr_idx = UNRESOLVED_TOKENS
                .iter()
                .filter_map(|w| lower.rfind(w))
                .max();
            outcome = match (res_idx, unr_idx) {
                (Some(r), Some(u)) if r > u => "resolved".to_string(),
                (Some(_), Some(_)) => "unresolved".to_string(),
                _ => "partial".to_string(),
            };
        }
    }
    // Override: if the LAST Bash tool call's result shows exit_code=0 → "resolved".
    if let Some(last_bash_ok) = last_bash_succeeded(turns) {
        if last_bash_ok {
            outcome = "resolved".to_string();
        }
    }
    outcome
}

/// Walk turns backward; return `Some(true)` if the last Bash tool call's
/// tool_result has a non-error, exit-zero output. `Some(false)` if the last
/// Bash call had an error. `None` if there was no Bash call at all.
fn last_bash_succeeded(turns: &[Turn]) -> Option<bool> {
    // We need to pair the LATEST Bash tool_call with its tool_result (by id).
    let mut latest_bash_id: Option<String> = None;
    'outer: for t in turns.iter().rev() {
        for tc in t.tool_calls.iter().rev() {
            if tc.name == "Bash" {
                latest_bash_id = Some(tc.id.clone());
                break 'outer;
            }
        }
    }
    let id = latest_bash_id?;
    // Find the matching tool_result in any later (chronological) turn.
    for t in turns.iter() {
        for r in &t.tool_results {
            if r.tool_use_id == id {
                if r.is_error {
                    return Some(false);
                }
                // Heuristic: exit code "0" or absence of "exit code" + no
                // "error"-ish markers → success.
                let lower = r.content.to_lowercase();
                if lower.contains("exit code 0") || lower.contains("process exited with code 0") {
                    return Some(true);
                }
                if lower.contains("error") || lower.contains("failed") {
                    return Some(false);
                }
                // Default: not-an-error result means it succeeded.
                return Some(true);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Arc
// ---------------------------------------------------------------------------

/// Tool sequence pattern. See module-level docs for the exact rules.
pub fn classify_arc(turns: &[Turn]) -> String {
    // Flat sequence of (tool_name, had_error_in_result) over all tool calls,
    // in chronological order. `had_error` is detected via matching tool_result.
    // Tuples (not a struct) so the pattern matchers below stay simple.
    let mut error_by_id: BTreeMap<String, bool> = BTreeMap::new();
    for t in turns {
        for r in &t.tool_results {
            // is_error from the explicit field OR error markers in content.
            let lower = r.content.to_lowercase();
            let textual_error = lower.contains("error")
                || lower.contains("failed")
                || lower.contains("exit code 1");
            error_by_id.insert(r.tool_use_id.clone(), r.is_error || textual_error);
        }
    }
    let mut steps: Vec<(String, bool)> = Vec::new();
    for t in turns {
        for tc in &t.tool_calls {
            let had_error = error_by_id.get(&tc.id).copied().unwrap_or(false);
            steps.push((tc.name.clone(), had_error));
        }
    }

    if steps.is_empty() {
        return "mixed".to_string();
    }

    let n = steps.len();
    let read = steps.iter().filter(|(name, _)| name == "Read").count();
    let edit_or_write = steps
        .iter()
        .filter(|(name, _)| is_edit_like(name))
        .count();
    let bash = steps.iter().filter(|(name, _)| name == "Bash").count();
    let bash_failed = steps
        .iter()
        .filter(|(name, err)| name == "Bash" && *err)
        .count();

    if matches_pattern_read_failbash_edit(&steps) && bash_failed > 0 {
        return "debug-fix".to_string();
    }
    if matches_pattern_read_edit_bash(&steps)
        && bash_failed == 0
        && read >= 1
        && edit_or_write >= 1
        && bash >= 1
    {
        return "fix".to_string();
    }
    // Mostly-edit, no Bash → impl.
    if edit_or_write * 2 >= n && bash == 0 {
        return "impl".to_string();
    }
    // Mostly-read, no Edit/Write → explore.
    if read * 2 >= n && edit_or_write == 0 {
        return "explore".to_string();
    }
    "mixed".to_string()
}

fn is_edit_like(name: &str) -> bool {
    matches!(name, "Edit" | "MultiEdit" | "Write")
}

/// Detect: at least one Read → at least one Edit → at least one Bash, in order.
fn matches_pattern_read_edit_bash(steps: &[(String, bool)]) -> bool {
    let mut saw_read = false;
    let mut saw_edit_after_read = false;
    for (name, _) in steps {
        if !saw_read && name == "Read" {
            saw_read = true;
        } else if saw_read && !saw_edit_after_read && is_edit_like(name) {
            saw_edit_after_read = true;
        } else if saw_read && saw_edit_after_read && name == "Bash" {
            return true;
        }
    }
    false
}

/// Detect: Read → failed Bash (one or more) → Edit, in order.
fn matches_pattern_read_failbash_edit(steps: &[(String, bool)]) -> bool {
    let mut saw_read = false;
    let mut saw_failed_bash = false;
    for (name, had_error) in steps {
        if !saw_read && name == "Read" {
            saw_read = true;
        } else if saw_read && !saw_failed_bash && name == "Bash" && *had_error {
            saw_failed_bash = true;
        } else if saw_read && saw_failed_bash && is_edit_like(name) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Topic
// ---------------------------------------------------------------------------

/// Number of leading words to pull from the first user message for the
/// fallback noun phrase. Stable, lowercased.
const TOPIC_NOUN_WORDS: usize = 4;

/// Derive a one-line topic label.
///
/// If `session.ai_title` is non-empty, return it. Otherwise build
/// `"<project_name> · <first_noun_phrase>"` from the first user message text.
pub fn derive_topic(session: &Session, turns: &[Turn]) -> String {
    if let Some(t) = session.ai_title.as_deref() {
        let t = t.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    let project = session.project_name.as_deref().unwrap_or("(unknown)");
    let phrase = first_noun_phrase(turns).unwrap_or_else(|| "(no description)".to_string());
    format!("{project} · {phrase}")
}

/// Take the first user message, strip code fences/blocks, lowercase, and
/// return the first `TOPIC_NOUN_WORDS` words.
fn first_noun_phrase(turns: &[Turn]) -> Option<String> {
    let first_user = turns.iter().find(|t| t.role == TurnRole::User)?;
    let text = strip_code_blocks(&first_user.text);
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_lowercase())
        .filter(|w| !w.is_empty())
        .take(TOPIC_NOUN_WORDS)
        .collect();
    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

/// Strip triple-backtick code blocks and inline code from a text string.
fn strip_code_blocks(s: &str) -> String {
    static FENCED: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?s)```[\w+-]*\n.*?```").unwrap());
    static INLINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`[^`]*`").unwrap());
    let no_fenced = FENCED.replace_all(s, " ");
    let no_inline = INLINE.replace_all(&no_fenced, " ");
    no_inline.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{EventCounts, ToolCall, ToolResult, TurnRole};
    use serde_json::json;

    fn make_session(ai_title: Option<&str>, project: Option<&str>) -> Session {
        Session {
            session_id: "test".to_string(),
            source_path: "/tmp/test.jsonl".into(),
            project_path: project.map(|p| format!("/Users/u/{p}")),
            project_name: project.map(|p| p.to_string()),
            git_branch: Some("main".to_string()),
            claude_version: None,
            ai_title: ai_title.map(|t| t.to_string()),
            start_time: None,
            end_time: None,
            turns: Vec::new(),
            event_counts: EventCounts::default(),
        }
    }

    fn make_turn(role: TurnRole, text: &str, tool_calls: Vec<ToolCall>) -> Turn {
        Turn {
            uuid: format!("u-{}", text.len()),
            parent_uuid: None,
            timestamp: None,
            role,
            is_sidechain: false,
            text: text.to_string(),
            tool_calls,
            tool_results: Vec::new(),
        }
    }

    fn tc(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall {
            id: format!("tc-{}-{}", name, input.to_string().len()),
            name: name.to_string(),
            input,
        }
    }

    // ---- intent --------------------------------------------------------

    #[test]
    fn t_intent_bash_dominant() {
        let mut turns = Vec::new();
        for _ in 0..6 {
            turns.push(make_turn(
                TurnRole::Assistant,
                "",
                vec![tc("Bash", json!({"command": "ls"}))],
            ));
        }
        turns.push(make_turn(
            TurnRole::Assistant,
            "",
            vec![tc("Read", json!({"file_path": "/a.rs"}))],
        ));
        assert_eq!(classify_intent(&turns), "build");
    }

    #[test]
    fn t_intent_edit_dominant() {
        let mut turns = Vec::new();
        for _ in 0..5 {
            turns.push(make_turn(
                TurnRole::Assistant,
                "",
                vec![tc("Edit", json!({"file_path": "/a.rs"}))],
            ));
        }
        turns.push(make_turn(
            TurnRole::Assistant,
            "",
            vec![tc("Read", json!({"file_path": "/b.rs"}))],
        ));
        assert_eq!(classify_intent(&turns), "impl");
    }

    #[test]
    fn t_intent_read_dominant() {
        let mut turns = Vec::new();
        for _ in 0..5 {
            turns.push(make_turn(
                TurnRole::Assistant,
                "",
                vec![tc("Read", json!({"file_path": "/a.rs"}))],
            ));
        }
        turns.push(make_turn(
            TurnRole::Assistant,
            "",
            vec![tc("Bash", json!({"command": "ls"}))],
        ));
        assert_eq!(classify_intent(&turns), "debug");
    }

    #[test]
    fn t_intent_mixed_when_no_dominance() {
        // 2 Bash, 2 Edit, 2 Read = perfectly balanced.
        let turns = vec![
            make_turn(TurnRole::Assistant, "", vec![tc("Bash", json!({"command": "ls"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Bash", json!({"command": "ls"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Edit", json!({"file_path": "/a.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Edit", json!({"file_path": "/a.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Read", json!({"file_path": "/a.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Read", json!({"file_path": "/a.rs"}))]),
        ];
        assert_eq!(classify_intent(&turns), "mixed");
    }

    #[test]
    fn t_intent_empty_session_mixed() {
        assert_eq!(classify_intent(&[]), "mixed");
    }

    // ---- entities ------------------------------------------------------

    #[test]
    fn t_entities_collect_paths() {
        let turns = vec![
            make_turn(
                TurnRole::Assistant,
                "see src/main.rs and tests/foo.rs for details",
                vec![tc("Read", json!({"file_path": "src/main.rs"}))],
            ),
            make_turn(
                TurnRole::Assistant,
                "",
                vec![tc("Edit", json!({"file_path": "src/main.rs"}))],
            ),
        ];
        let ents = extract_entities(&turns);
        // src/main.rs appears 3x (Read input, Edit input, free text), tests/foo.rs once.
        assert_eq!(ents.first().map(|s| s.as_str()), Some("src/main.rs"));
        assert!(ents.iter().any(|e| e == "tests/foo.rs"));
    }

    #[test]
    fn t_entities_capture_bash_command() {
        let turns = vec![make_turn(
            TurnRole::Assistant,
            "",
            vec![tc("Bash", json!({"command": "cargo build --release"}))],
        )];
        let ents = extract_entities(&turns);
        assert!(
            ents.iter().any(|e| e == "cargo"),
            "first Bash word should be captured: {ents:?}"
        );
    }

    #[test]
    fn t_entities_top_n_cap() {
        // 15 unique paths — only top 10 returned.
        let calls: Vec<ToolCall> = (0..15)
            .map(|i| tc("Read", json!({"file_path": format!("file{}.rs", i)})))
            .collect();
        let turns = vec![make_turn(TurnRole::Assistant, "", calls)];
        let ents = extract_entities(&turns);
        assert_eq!(ents.len(), 10);
    }

    #[test]
    fn t_entities_empty_when_no_paths() {
        let turns = vec![make_turn(TurnRole::User, "hello world", vec![])];
        assert!(extract_entities(&turns).is_empty());
    }

    // ---- outcome -------------------------------------------------------

    #[test]
    fn t_outcome_resolved_from_text() {
        let turns = vec![make_turn(
            TurnRole::Assistant,
            "All tests passed and the bug is fixed.",
            vec![],
        )];
        assert_eq!(classify_outcome(&turns), "resolved");
    }

    #[test]
    fn t_outcome_unresolved_from_text() {
        let turns = vec![make_turn(
            TurnRole::Assistant,
            "Build failed and I cannot find the missing module.",
            vec![],
        )];
        assert_eq!(classify_outcome(&turns), "unresolved");
    }

    #[test]
    fn t_outcome_partial_when_no_signal() {
        let turns = vec![make_turn(
            TurnRole::Assistant,
            "I made some changes to the auth module.",
            vec![],
        )];
        assert_eq!(classify_outcome(&turns), "partial");
    }

    #[test]
    fn t_outcome_bash_exit_zero_overrides() {
        // Assistant text says "error" (would be unresolved) but the last Bash
        // exited 0 → outcome must flip to "resolved".
        let mut bash_call = tc("Bash", json!({"command": "cargo test"}));
        bash_call.id = "call-1".to_string();
        let last_assistant = make_turn(TurnRole::Assistant, "Hmm, an error?", vec![bash_call]);
        let result_turn = Turn {
            uuid: "r".to_string(),
            parent_uuid: None,
            timestamp: None,
            role: TurnRole::User,
            is_sidechain: false,
            text: String::new(),
            tool_calls: vec![],
            tool_results: vec![ToolResult {
                tool_use_id: "call-1".to_string(),
                content: "process exited with code 0".to_string(),
                is_error: false,
            }],
        };
        let turns = vec![last_assistant, result_turn];
        assert_eq!(classify_outcome(&turns), "resolved");
    }

    // ---- arc -----------------------------------------------------------

    #[test]
    fn t_arc_fix_pattern() {
        let turns = vec![
            make_turn(TurnRole::Assistant, "", vec![tc("Read", json!({"file_path": "/a.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Edit", json!({"file_path": "/a.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Bash", json!({"command": "cargo test"}))]),
        ];
        // No failure markers in any tool_result.
        assert_eq!(classify_arc(&turns), "fix");
    }

    #[test]
    fn t_arc_debug_fix_pattern() {
        let mut bash_call = tc("Bash", json!({"command": "cargo test"}));
        bash_call.id = "call-bash".to_string();
        let mut turns = vec![
            make_turn(TurnRole::Assistant, "", vec![tc("Read", json!({"file_path": "/a.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![bash_call]),
            make_turn(TurnRole::Assistant, "", vec![tc("Edit", json!({"file_path": "/a.rs"}))]),
        ];
        // Inject a failing tool_result for the Bash call.
        turns[1].tool_results.push(ToolResult {
            tool_use_id: "call-bash".to_string(),
            content: "error: tests failed".to_string(),
            is_error: true,
        });
        assert_eq!(classify_arc(&turns), "debug-fix");
    }

    #[test]
    fn t_arc_impl_pattern() {
        let turns = vec![
            make_turn(TurnRole::Assistant, "", vec![tc("Edit", json!({"file_path": "/a.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Edit", json!({"file_path": "/b.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Write", json!({"file_path": "/c.rs"}))]),
        ];
        assert_eq!(classify_arc(&turns), "impl");
    }

    #[test]
    fn t_arc_explore_pattern() {
        let turns = vec![
            make_turn(TurnRole::Assistant, "", vec![tc("Read", json!({"file_path": "/a.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Read", json!({"file_path": "/b.rs"}))]),
            make_turn(TurnRole::Assistant, "", vec![tc("Grep", json!({"pattern": "foo"}))]),
        ];
        // 2/3 are Read, no Edit/Write.
        assert_eq!(classify_arc(&turns), "explore");
    }

    #[test]
    fn t_arc_mixed_default() {
        // One of each, no clear pattern.
        let turns = vec![make_turn(
            TurnRole::Assistant,
            "",
            vec![
                tc("WebFetch", json!({"url": "https://example.com"})),
                tc("Task", json!({"prompt": "do stuff"})),
            ],
        )];
        assert_eq!(classify_arc(&turns), "mixed");
    }

    // ---- topic ---------------------------------------------------------

    #[test]
    fn t_topic_uses_ai_title_when_present() {
        let session = make_session(Some("Migrating to Qdrant v1.18"), Some("memex"));
        let turns = vec![make_turn(TurnRole::User, "let's start", vec![])];
        assert_eq!(derive_topic(&session, &turns), "Migrating to Qdrant v1.18");
    }

    #[test]
    fn t_topic_falls_back_to_first_user() {
        let session = make_session(None, Some("memex"));
        let turns = vec![make_turn(
            TurnRole::User,
            "Implement the Codex parser today",
            vec![],
        )];
        assert_eq!(
            derive_topic(&session, &turns),
            "memex · implement the codex parser"
        );
    }

    #[test]
    fn t_topic_strips_code_blocks() {
        let session = make_session(None, Some("proj"));
        let turns = vec![make_turn(
            TurnRole::User,
            "```rust\nfn main(){}\n``` fix this bug please now",
            vec![],
        )];
        let topic = derive_topic(&session, &turns);
        assert!(
            topic.starts_with("proj · fix this bug"),
            "code-fence content must be stripped: {topic}"
        );
    }

    #[test]
    fn t_topic_handles_empty_session() {
        let session = make_session(None, None);
        let turns: Vec<Turn> = vec![];
        let topic = derive_topic(&session, &turns);
        assert_eq!(topic, "(unknown) · (no description)");
    }

    // ---- determinism ---------------------------------------------------

    #[test]
    fn t_enrich_determinism() {
        // Same input → byte-identical output across two calls.
        let session = make_session(Some("Demo title"), Some("memex"));
        let turns = vec![
            make_turn(
                TurnRole::User,
                "fix the parser please src/parser.rs",
                vec![],
            ),
            make_turn(
                TurnRole::Assistant,
                "All tests passed.",
                vec![
                    tc("Read", json!({"file_path": "src/parser.rs"})),
                    tc("Edit", json!({"file_path": "src/parser.rs"})),
                    tc("Bash", json!({"command": "cargo test --release"})),
                ],
            ),
        ];
        let a = enrich(&session, &turns);
        let b = enrich(&session, &turns);
        assert_eq!(a, b, "enrich must be deterministic");
    }

    #[test]
    fn t_enrich_full_session_smoke() {
        // End-to-end: every field non-empty, valid string values.
        let session = make_session(None, Some("memex"));
        let mut bash_call = tc("Bash", json!({"command": "cargo build"}));
        bash_call.id = "b1".to_string();
        let turns = vec![
            make_turn(
                TurnRole::User,
                "build the project src/lib.rs",
                vec![],
            ),
            make_turn(
                TurnRole::Assistant,
                "Build succeeded — everything is fixed.",
                vec![bash_call.clone()],
            ),
            Turn {
                uuid: "rt".into(),
                parent_uuid: None,
                timestamp: None,
                role: TurnRole::User,
                is_sidechain: false,
                text: String::new(),
                tool_calls: vec![],
                tool_results: vec![ToolResult {
                    tool_use_id: "b1".into(),
                    content: "exit code 0".into(),
                    is_error: false,
                }],
            },
        ];
        let out = enrich(&session, &turns);
        // Intent should be "build" (1/1 = 100% Bash) → > 40% threshold.
        assert_eq!(out.intent, "build");
        assert_eq!(out.outcome, "resolved");
        assert!(out.topic.contains("memex"));
        assert!(!out.entities.is_empty());
    }
}
