//! Shared session-root resolution and scanning.
//!
//! GUI commands and the MCP server must agree on where Claude, Codex, and
//! legacy transcript sessions live. Keep that policy here so Windows/home-dir
//! handling and parser routing cannot drift between entry points.

use std::path::{Component, Path, PathBuf};

use anyhow::Result;

use crate::parser;

fn home_relative(parts: &[&str]) -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(PathBuf::new);
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

/// Route a single explicit root to the right parser.
///
/// Codex roots are detected both by canonical path components and by rollout
/// filename sniffing, so symlinks or alternate mounts still get parsed with the
/// correct envelope.
pub fn scan_root_routed(root: &Path) -> Result<Vec<parser::Session>> {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if has_component_pair(&canonical, ".codex", "sessions") {
        return crate::codex_parser::scan_codex_dir(&canonical);
    }

    let first_rollout = walkdir::WalkDir::new(&canonical)
        .max_depth(4)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .find(|entry| {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            name.starts_with("rollout-") && name.ends_with(".jsonl")
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
