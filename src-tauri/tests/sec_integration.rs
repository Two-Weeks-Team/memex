//! Integration: exercise sec::validate_session_path against real $HOME roots
//! when they exist, and skip gracefully otherwise.

use memex_lib::sec::{validate_session_path, SandboxRoot};
use std::path::PathBuf;

#[test]
fn it_sandbox_from_env_succeeds_if_any_root_exists() {
    // CI may or may not have these dirs. We can at least call it.
    let home = std::env::var_os("HOME").unwrap();
    let claude = PathBuf::from(&home).join(".claude/projects");
    let codex = PathBuf::from(&home).join(".codex/sessions");
    let any_exists = claude.exists() || codex.exists();
    let result = SandboxRoot::from_env();
    if any_exists {
        assert!(result.is_ok());
    } else {
        assert!(result.is_err());
    }
}

#[test]
fn it_rejects_etc_passwd() {
    // Always-present path that's never in the sandbox.
    let p = PathBuf::from("/etc/passwd");
    let r = validate_session_path(&p);
    if SandboxRoot::from_env().is_ok() {
        assert!(r.is_err());
    }
}
