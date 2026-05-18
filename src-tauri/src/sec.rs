//! Security primitives: sandboxed session-path validation across multi-agent roots.
//!
//! Memex indexes sessions from BOTH ~/.claude/projects/ (Claude Code) and
//! ~/.codex/sessions/ (Codex CLI). Every IPC entry point that resolves a
//! filesystem path from Qdrant payload MUST canonicalize it and confirm it
//! lives inside one of these roots — otherwise a tampered payload could read
//! arbitrary files.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SourceAgent {
    ClaudeCode,
    Codex,
}

impl SourceAgent {
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceAgent::ClaudeCode => "claude_code",
            SourceAgent::Codex => "codex",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxRoot {
    canonical_roots: Vec<(SourceAgent, PathBuf)>,
}

impl SandboxRoot {
    /// Discover roots from $HOME. Tolerates either agent being absent so
    /// users with only one of the two can still run Memex.
    pub fn from_env() -> Result<Self> {
        let home = std::env::var_os("HOME").context("HOME unset")?;
        let home = PathBuf::from(home);
        let candidates = [
            (SourceAgent::ClaudeCode, home.join(".claude/projects")),
            (SourceAgent::Codex, home.join(".codex/sessions")),
        ];
        let canonical_roots: Vec<(SourceAgent, PathBuf)> = candidates
            .into_iter()
            .filter_map(|(a, p)| p.canonicalize().ok().map(|c| (a, c)))
            .collect();
        if canonical_roots.is_empty() {
            bail!("no valid session root found (neither ~/.claude/projects nor ~/.codex/sessions exists)");
        }
        Ok(Self { canonical_roots })
    }

    /// Build a SandboxRoot from explicit canonical paths — for tests that
    /// don't want to touch the real $HOME.
    #[cfg(test)]
    pub fn from_roots(roots: Vec<(SourceAgent, PathBuf)>) -> Self {
        Self { canonical_roots: roots }
    }

    /// Returns the canonical path if `p` (after canonicalize) lives inside ANY
    /// configured root. Rejects NUL bytes pre-canonicalize because Rust's
    /// canonicalize would otherwise pass them straight to the syscall.
    pub fn contains(&self, p: &Path) -> Result<PathBuf> {
        let s = p.as_os_str();
        if s.is_empty() {
            bail!("path is empty");
        }
        // NUL byte check on the raw bytes — most reliable on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            if s.as_bytes().contains(&0) {
                bail!("path contains NUL byte: {}", p.display());
            }
        }
        let canon = p
            .canonicalize()
            .with_context(|| format!("canonicalize {}", p.display()))?;
        for (_, root) in &self.canonical_roots {
            if canon.starts_with(root) {
                return Ok(canon);
            }
        }
        Err(anyhow!("path outside sandbox: {}", canon.display()))
    }

    /// Returns Some(agent) if `p` is contained AND identifies which root
    /// matched; None otherwise. Does not throw — useful for routing decisions.
    pub fn detect_agent(&self, p: &Path) -> Option<SourceAgent> {
        let canon = p.canonicalize().ok()?;
        for (agent, root) in &self.canonical_roots {
            if canon.starts_with(root) {
                return Some(*agent);
            }
        }
        None
    }

    /// Public read of configured roots — used by the scanner.
    pub fn roots(&self) -> impl Iterator<Item = (SourceAgent, &Path)> {
        self.canonical_roots
            .iter()
            .map(|(a, p)| (*a, p.as_path()))
    }
}

/// Convenience wrapper used by IPC entry points. Reads SandboxRoot from env
/// fresh each call (cheap — canonicalize on directories is one stat each).
pub fn validate_session_path(p: &Path) -> Result<PathBuf> {
    SandboxRoot::from_env()?.contains(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Build a tempdir-backed SandboxRoot with both roots fabricated so the
    /// tests don't depend on the real $HOME contents.
    fn make_sandbox() -> (TempDir, SandboxRoot) {
        let td = TempDir::new().expect("tempdir");
        let claude_root = td.path().join("claude_projects");
        let codex_root = td.path().join("codex_sessions");
        fs::create_dir_all(&claude_root).unwrap();
        fs::create_dir_all(&codex_root).unwrap();
        let sb = SandboxRoot::from_roots(vec![
            (SourceAgent::ClaudeCode, claude_root.canonicalize().unwrap()),
            (SourceAgent::Codex, codex_root.canonicalize().unwrap()),
        ]);
        (td, sb)
    }

    #[test]
    fn t_valid_claude_session_path() {
        let (td, sb) = make_sandbox();
        let p = td.path().join("claude_projects/abc/sess.jsonl");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "{}\n").unwrap();
        assert!(sb.contains(&p).is_ok());
        assert_eq!(sb.detect_agent(&p), Some(SourceAgent::ClaudeCode));
    }

    #[test]
    fn t_valid_codex_session_path() {
        let (td, sb) = make_sandbox();
        let p = td.path().join("codex_sessions/2026/05/18/rollout-x.jsonl");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "{}\n").unwrap();
        assert!(sb.contains(&p).is_ok());
        assert_eq!(sb.detect_agent(&p), Some(SourceAgent::Codex));
    }

    #[test]
    fn t_path_outside_sandbox_etc() {
        let (_td, sb) = make_sandbox();
        // /etc/passwd exists on macOS/Linux but isn't in the sandbox.
        let p = PathBuf::from("/etc/passwd");
        assert!(sb.contains(&p).is_err());
        assert!(sb.detect_agent(&p).is_none());
    }

    #[test]
    fn t_path_outside_both_tmp() {
        let (_td, sb) = make_sandbox();
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("foo.jsonl");
        fs::write(&p, "x").unwrap();
        assert!(sb.contains(&p).is_err());
    }

    #[test]
    fn t_path_traversal_dotdot() {
        let (td, sb) = make_sandbox();
        // Construct a file inside, then traverse out via .. — canonicalize
        // should resolve to outside the sandbox.
        let inside = td.path().join("claude_projects/abc/sess.jsonl");
        fs::create_dir_all(inside.parent().unwrap()).unwrap();
        fs::write(&inside, "x").unwrap();
        let traversal = td
            .path()
            .join("claude_projects/abc/../../outside.jsonl");
        // Create the 'outside' file so canonicalize succeeds; the security
        // assertion is that canonical form is outside both roots.
        fs::write(td.path().join("outside.jsonl"), "x").unwrap();
        assert!(sb.contains(&traversal).is_err());
    }

    #[test]
    fn t_symlink_outside() {
        let (td, sb) = make_sandbox();
        let outside_dir = TempDir::new().unwrap();
        let outside_file = outside_dir.path().join("secret.txt");
        fs::write(&outside_file, "secret").unwrap();
        let link = td.path().join("claude_projects/link.jsonl");
        std::os::unix::fs::symlink(&outside_file, &link).unwrap();
        assert!(sb.contains(&link).is_err(), "symlink escaping sandbox must be rejected");
    }

    #[test]
    fn t_symlink_inside() {
        let (td, sb) = make_sandbox();
        let real = td.path().join("claude_projects/def/foo.jsonl");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "x").unwrap();
        let link = td.path().join("claude_projects/abc/link.jsonl");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(sb.contains(&link).is_ok(), "internal symlink should be accepted");
    }

    #[test]
    fn t_nul_byte_path() {
        let (_td, sb) = make_sandbox();
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let p = PathBuf::from(OsString::from_vec(b"foo\0bar.jsonl".to_vec()));
        assert!(sb.contains(&p).is_err());
    }

    #[test]
    fn t_empty_string() {
        let (_td, sb) = make_sandbox();
        let p = PathBuf::from("");
        assert!(sb.contains(&p).is_err());
    }

    #[test]
    fn t_nonexistent_path() {
        let (td, sb) = make_sandbox();
        let p = td.path().join("claude_projects/does_not_exist.jsonl");
        assert!(sb.contains(&p).is_err());
    }

    #[test]
    fn t_canonical_idempotent() {
        let (td, sb) = make_sandbox();
        let p = td.path().join("claude_projects/abc/sess.jsonl");
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "x").unwrap();
        let c1 = sb.contains(&p).unwrap();
        let c2 = sb.contains(&c1).unwrap();
        assert_eq!(c1, c2);
    }

    #[test]
    fn t_graceful_codex_missing() {
        // Only Claude root exists. SandboxRoot should still construct.
        let td = TempDir::new().unwrap();
        let claude_root = td.path().join("claude_projects");
        fs::create_dir_all(&claude_root).unwrap();
        let sb = SandboxRoot::from_roots(vec![(
            SourceAgent::ClaudeCode,
            claude_root.canonicalize().unwrap(),
        )]);
        let p = td.path().join("claude_projects/x.jsonl");
        fs::write(&p, "x").unwrap();
        assert!(sb.contains(&p).is_ok());
    }

    #[test]
    fn t_source_agent_as_str() {
        assert_eq!(SourceAgent::ClaudeCode.as_str(), "claude_code");
        assert_eq!(SourceAgent::Codex.as_str(), "codex");
    }

    #[test]
    fn t_no_panic_on_arbitrary_bytes() {
        let (_td, sb) = make_sandbox();
        // Random byte sequences that aren't valid paths shouldn't panic.
        for bytes in [b"\xff\xfe\xff".as_ref(), b"a\nb\rc".as_ref(), b"\x7f".as_ref()] {
            use std::ffi::OsString;
            use std::os::unix::ffi::OsStringExt;
            let p = PathBuf::from(OsString::from_vec(bytes.to_vec()));
            let _ = sb.contains(&p); // must not panic
            let _ = sb.detect_agent(&p); // must not panic
        }
    }
}
