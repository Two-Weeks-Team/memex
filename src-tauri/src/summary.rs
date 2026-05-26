//! `SessionSummary` — a Tauri-free, serializable summary of a parsed session.
//!
//! Lives in its own module (rather than `commands.rs`) so it is available to
//! both the GUI build (Tauri commands) and the headless `web`/`mcp` builds,
//! which must compile without the `tauri` dependency. `commands.rs` re-exports
//! it so existing `commands::SessionSummary` paths keep working.

use crate::parser;

/// A lightweight, parser-only summary of a session — no Qdrant calls. Used by
/// the Time Machine stack on boot, the JSON API (`web`), and the MCP server.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    pub project_name: String,
    pub project_path: String,
    pub git_branch: String,
    pub ai_title: String,
    pub start_iso: String,
    pub end_iso: String,
    pub user_turns: usize,
    pub assistant_turns: usize,
    pub tool_count: usize,
    pub has_errors: bool,
}

impl From<parser::Session> for SessionSummary {
    fn from(s: parser::Session) -> Self {
        let tool_count: usize = s.turns.iter().map(|t| t.tool_calls.len()).sum();
        let has_errors = s
            .turns
            .iter()
            .any(|t| t.tool_results.iter().any(|r| r.is_error));
        Self {
            session_id: s.session_id,
            project_name: s.project_name.unwrap_or_default(),
            project_path: s.project_path.unwrap_or_default(),
            git_branch: s.git_branch.unwrap_or_default(),
            ai_title: s.ai_title.unwrap_or_default(),
            start_iso: s.start_time.map(|t| t.to_rfc3339()).unwrap_or_default(),
            end_iso: s.end_time.map(|t| t.to_rfc3339()).unwrap_or_default(),
            user_turns: s.event_counts.user,
            assistant_turns: s.event_counts.assistant,
            tool_count,
            has_errors,
        }
    }
}
