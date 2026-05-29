//! Shared session-root resolution and scanning.
//!
//! GUI commands and the MCP server must agree on where Claude, Codex, and
//! legacy transcript sessions live. Keep that policy here so Windows/home-dir
//! handling and parser routing cannot drift between entry points.

use std::path::{Component, Path, PathBuf};

use anyhow::Result;

use crate::parser;

fn home_relative(parts: &[&str]) -> PathBuf {
    let mut path =
        dirs::home_dir().expect("could not resolve home directory for Memex session roots");
    for part in parts {
        path.push(part);
    }
    path
}

pub fn default_projects_root() -> PathBuf {
    home_relative(&[".claude", "projects"])
}

pub fn default_codex_root() -> PathBuf {
    home_relative(&[".codex", "sessions"])
}

pub fn default_transcripts_root() -> PathBuf {
    home_relative(&[".claude", "transcripts"])
}

pub fn default_history_path() -> PathBuf {
    home_relative(&[".claude", "history.jsonl"])
}

fn has_component_pair(path: &Path, first: &str, second: &str) -> bool {
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();

    parts
        .windows(2)
        .any(|window| window[0] == first && window[1] == second)
}

fn is_rollout_jsonl_name(name: &str) -> bool {
    name.get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("rollout-"))
        && name
            .get(name.len().saturating_sub(6)..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".jsonl"))
}

fn is_under_existing_root(path: &Path, root: &Path) -> bool {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let Ok(canonical_root) = root.canonicalize() else {
        return false;
    };
    canonical.starts_with(canonical_root)
}

pub fn is_legacy_transcript_path(path: &Path) -> bool {
    is_under_existing_root(path, &default_transcripts_root())
}

/// Route one already-validated session file to the parser for its envelope.
///
/// `source_agent` distinguishes Codex from Claude Code, while the legacy
/// `~/.claude/transcripts/ses_*.jsonl` corpus needs a Claude-specific parser
/// because those files do not use the modern project JSONL envelope.
pub fn parse_session_routed(source_agent: &str, path: &Path) -> Result<parser::Session> {
    if source_agent == "codex" {
        crate::codex_parser::parse_codex_session(path)
    } else if is_legacy_transcript_path(path) {
        parser::parse_transcript_session(path)
    } else {
        parser::parse_session(path)
    }
}

/// Route a single explicit root to the right parser.
///
/// Codex roots are detected both by canonical path components and by rollout
/// filename sniffing, so symlinks or alternate mounts still get parsed with the
/// correct envelope.
pub fn scan_root_routed(root: &Path) -> Result<Vec<parser::Session>> {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if is_under_existing_root(&canonical, &default_transcripts_root()) {
        return parser::scan_transcripts_dir(&canonical);
    }
    if has_component_pair(&canonical, ".codex", "sessions") {
        return crate::codex_parser::scan_codex_dir(&canonical);
    }

    let first_rollout = walkdir::WalkDir::new(&canonical)
        .max_depth(4)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .find(|entry| {
            let name = entry.file_name().to_string_lossy();
            is_rollout_jsonl_name(&name)
        });
    if first_rollout.is_some() {
        return crate::codex_parser::scan_codex_dir(&canonical);
    }

    parser::scan_dir(&canonical)
}

/// Scan Claude projects, Codex sessions, and legacy Claude transcripts.
pub fn scan_all_roots(log_prefix: &str) -> Result<Vec<parser::Session>> {
    let mut all = Vec::new();

    let claude_root = default_projects_root();
    if claude_root.exists() {
        match parser::scan_dir(&claude_root) {
            Ok(mut sessions) => all.append(&mut sessions),
            Err(e) => eprintln!("{log_prefix} claude root scan: {e:#}"),
        }
    }

    let codex_root = default_codex_root();
    if codex_root.exists() {
        match crate::codex_parser::scan_codex_dir(&codex_root) {
            Ok(mut sessions) => all.append(&mut sessions),
            Err(e) => eprintln!("{log_prefix} codex root scan: {e:#}"),
        }
    }

    let transcripts_root = default_transcripts_root();
    if transcripts_root.exists() {
        match parser::scan_transcripts_dir(&transcripts_root) {
            Ok(mut sessions) => all.append(&mut sessions),
            Err(e) => eprintln!("{log_prefix} legacy transcripts scan: {e:#}"),
        }
    }

    if all.is_empty() {
        anyhow::bail!(
            "no sessions found — neither {} nor {} nor {} contained parseable rollouts",
            claude_root.display(),
            codex_root.display(),
            transcripts_root.display(),
        );
    }

    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn t_rollout_sniff_is_case_insensitive_without_lowercase_copy() {
        assert!(is_rollout_jsonl_name("rollout-2026.jsonl"));
        assert!(is_rollout_jsonl_name("ROLLOUT-2026.JSONL"));
        assert!(!is_rollout_jsonl_name("session-2026.jsonl"));
        assert!(!is_rollout_jsonl_name("rollout-2026.txt"));
    }

    #[test]
    fn t_under_existing_root_detects_nested_paths() {
        let td = TempDir::new().expect("tempdir");
        let root = td.path().join("transcripts");
        let nested = root.join("ses_demo.jsonl");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&nested, "{}\n").unwrap();
        assert!(is_under_existing_root(&nested, &root));
        assert!(!is_under_existing_root(td.path(), &root));
    }
}
