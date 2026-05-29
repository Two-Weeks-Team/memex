//! Snapshot path sandbox + signed-envelope verification.
//!
//! The signature is a sidecar `.sig` file (NOT embedded in the snapshot blob),
//! which keeps the existing indexer::snapshot_export/import flow intact and
//! makes legacy snapshots (no sig) detectable.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};

/// Lowercase hex encoding of a byte slice. sha2 0.11 (digest 0.11) returns a
/// `hybrid_array::Array` from `finalize()` which — unlike digest 0.10's
/// `GenericArray` — no longer implements `LowerHex`, so `format!("{:x}", …)`
/// stopped compiling. Encode the bytes explicitly instead.
fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub const SNAPSHOT_EXT: &str = "snapshot";
pub const SIG_EXT: &str = "sig"; // → <name>.snapshot.sig

pub const CURRENT_SCHEMA_VERSION: u32 = 3;
pub const CURRENT_QDRANT_VERSION: &str = "1.18.0";
pub const ISSUER: &str = concat!("memex/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOp {
    Export,
    Import,
}

#[derive(Debug, Clone)]
pub struct SnapshotSandbox {
    root: PathBuf, // canonical
}

impl SnapshotSandbox {
    /// Build the default sandbox at the platform-appropriate per-user data
    /// directory (`~/Library/Application Support/dev.sgwannabe.memex/snapshots`
    /// on macOS, `$XDG_DATA_HOME/dev.sgwannabe.memex/snapshots` on Linux,
    /// `%APPDATA%\dev.sgwannabe.memex\snapshots` on Windows).
    ///
    /// PORTABILITY (Gemini review on PR #2, snapshot.rs:35): replaced the
    /// hardcoded macOS `$HOME/Library/Application Support/...` path with
    /// `dirs::data_dir()` so the snapshot directory lives in the platform's
    /// canonical app-data location everywhere. macOS resolves it back to
    /// the same `Library/Application Support` location, but Linux/Windows
    /// get sensible defaults out of the box.
    pub fn from_env() -> Result<Self> {
        let root = dirs::data_dir()
            .context("could not resolve platform data directory")?
            .join("dev.sgwannabe.memex/snapshots");
        std::fs::create_dir_all(&root)
            .with_context(|| format!("create_dir_all {}", root.display()))?;
        Ok(Self {
            root: root
                .canonicalize()
                .with_context(|| format!("canonicalize {}", root.display()))?,
        })
    }

    /// Construct a SnapshotSandbox with an explicit canonical root path.
    /// Exposed for tests (unit + integration); not part of the stable API.
    #[doc(hidden)]
    pub fn with_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// Validate a snapshot path for the given operation. Returns the canonical
    /// path on success.
    pub fn validate_path(&self, p: &Path, op: SnapshotOp) -> Result<PathBuf> {
        if p.as_os_str().is_empty() {
            bail!("snapshot path is empty");
        }
        // Reject NUL early.
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            if p.as_os_str().as_bytes().contains(&0) {
                bail!("snapshot path contains NUL byte");
            }
        }
        // Reject literal `..` segments. canonicalize would resolve them, but
        // for export the file doesn't exist yet, so we check the textual form
        // too.
        for c in p.components() {
            if matches!(c, std::path::Component::ParentDir) {
                bail!("snapshot path contains '..' traversal");
            }
        }
        // Resolve via parent (path may not exist yet on export).
        let parent = p
            .parent()
            .ok_or_else(|| anyhow!("snapshot path has no parent"))?;
        let parent_canon = parent
            .canonicalize()
            .with_context(|| format!("parent canonicalize {}", parent.display()))?;
        if !parent_canon.starts_with(&self.root) {
            bail!(
                "snapshot path outside sandbox: parent {} not in {}",
                parent_canon.display(),
                self.root.display()
            );
        }
        let fname = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("snapshot path has no filename"))?;
        // .snapshot extension required (matched on the final segment).
        if !fname.ends_with(&format!(".{SNAPSHOT_EXT}")) {
            bail!(
                "snapshot extension must be .{SNAPSHOT_EXT}: {}",
                p.display()
            );
        }
        let canonical = parent_canon.join(fname);
        match op {
            SnapshotOp::Export => {
                if canonical.exists() {
                    // SECURITY HARDENING (Codex review on PR #2, snapshot.rs:99):
                    // even on Export the file *can* exist if a previous
                    // export was left in place; if so, canonicalize the full
                    // path so a symlink-on-disk pointing outside the sandbox
                    // is rejected here rather than silently followed. We
                    // also bail because Export onto an existing file would
                    // overwrite — the original intent.
                    let full = canonical
                        .canonicalize()
                        .with_context(|| format!("canonicalize {}", canonical.display()))?;
                    if !full.starts_with(&self.root) {
                        bail!(
                            "snapshot file escapes sandbox via symlink: {} → {}",
                            canonical.display(),
                            full.display()
                        );
                    }
                    bail!("snapshot already exists: {}", canonical.display());
                }
            }
            SnapshotOp::Import => {
                if !canonical.exists() {
                    bail!("snapshot not found: {}", canonical.display());
                }
                // SECURITY HARDENING (Codex review on PR #2, snapshot.rs:99):
                // for Import we must fully canonicalize the target — the
                // previous logic only canonicalized the parent directory,
                // then naively re-joined the file name, so an in-sandbox
                // symlink (`snapshots/ok.snapshot -> /tmp/evil.snapshot`)
                // bypassed containment entirely (`SignedEnvelope::verify`
                // and `snapshot_import` would happily follow it). Resolving
                // the full path lets us reject the symlink target if it
                // escapes the sandbox root.
                let full = canonical
                    .canonicalize()
                    .with_context(|| format!("canonicalize {}", canonical.display()))?;
                if !full.starts_with(&self.root) {
                    bail!(
                        "snapshot file escapes sandbox via symlink: {} → {}",
                        canonical.display(),
                        full.display()
                    );
                }
                return Ok(full);
            }
        }
        Ok(canonical)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Signature {
    pub sha256: String, // hex, lowercase
    pub issued_by: String,
    pub issued_at: String, // RFC 3339
    pub schema_version: u32,
    pub qdrant_version: String,
}

#[derive(Debug)]
pub enum VerifyOutcome {
    Ok,
    LegacyNoSignature,
    WarnSchemaMismatch { expected: u32, found: u32 },
    WarnQdrantMinor { expected: String, found: String },
}

pub struct SignedEnvelope;

impl SignedEnvelope {
    pub fn sig_path_for(snapshot: &Path) -> PathBuf {
        let mut s = snapshot.as_os_str().to_owned();
        s.push(".");
        s.push(SIG_EXT);
        PathBuf::from(s)
    }

    /// Compute SHA-256 of the snapshot blob, write the sidecar .sig.
    pub fn sign(snapshot_path: &Path) -> Result<Signature> {
        let bytes = std::fs::read(snapshot_path)
            .with_context(|| format!("read {}", snapshot_path.display()))?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let sha256 = to_hex(&hasher.finalize());
        let issued_at = chrono::DateTime::<chrono::Utc>::from(SystemTime::now())
            .to_rfc3339();
        let sig = Signature {
            sha256,
            issued_by: ISSUER.to_string(),
            issued_at,
            schema_version: CURRENT_SCHEMA_VERSION,
            qdrant_version: CURRENT_QDRANT_VERSION.to_string(),
        };
        let sig_path = Self::sig_path_for(snapshot_path);
        let sig_json = serde_json::to_vec_pretty(&sig)?;
        std::fs::write(&sig_path, sig_json)
            .with_context(|| format!("write sig {}", sig_path.display()))?;
        Ok(sig)
    }

    /// Verify a snapshot against its sidecar .sig. Returns:
    ///   Ok(Ok)                           — valid
    ///   Ok(LegacyNoSignature)             — no .sig present (warn, allow)
    ///   Ok(WarnSchemaMismatch{..})        — schema differs (warn, allow)
    ///   Ok(WarnQdrantMinor{..})           — minor version drift (warn, allow)
    ///   Err(..)                           — tampering / major mismatch / malformed
    pub fn verify(snapshot_path: &Path) -> Result<VerifyOutcome> {
        let sig_path = Self::sig_path_for(snapshot_path);
        if !sig_path.exists() {
            return Ok(VerifyOutcome::LegacyNoSignature);
        }
        let sig_bytes = std::fs::read(&sig_path)
            .with_context(|| format!("read sig {}", sig_path.display()))?;
        let sig: Signature = serde_json::from_slice(&sig_bytes)
            .map_err(|e| anyhow!("malformed signature: {e}"))?;

        // SHA-256 check.
        let blob = std::fs::read(snapshot_path)
            .with_context(|| format!("read {}", snapshot_path.display()))?;
        let mut hasher = Sha256::new();
        hasher.update(&blob);
        let actual = to_hex(&hasher.finalize());
        if actual != sig.sha256 {
            bail!("sha256 mismatch: actual {} != expected {}", actual, sig.sha256);
        }

        // Qdrant major version check.
        let (sig_major, sig_minor) = parse_semver_major_minor(&sig.qdrant_version)
            .ok_or_else(|| anyhow!("malformed qdrant_version in sig: {}", sig.qdrant_version))?;
        let (cur_major, cur_minor) = parse_semver_major_minor(CURRENT_QDRANT_VERSION)
            .expect("CURRENT_QDRANT_VERSION constant is malformed");
        if sig_major != cur_major {
            bail!(
                "qdrant major mismatch: snapshot {} vs current {}",
                sig.qdrant_version,
                CURRENT_QDRANT_VERSION
            );
        }
        // PRIORITY FIX (CodeRabbit PR #2 review, snapshot.rs:208): when BOTH
        // schema_version and qdrant minor drift, prefer surfacing the schema
        // mismatch first because it's the higher-risk signal — a schema
        // mismatch can corrupt payload semantics, whereas a minor Qdrant
        // delta is usually safe-on-best-effort. Previously, the minor check
        // ran first and returned early, hiding any concurrent schema drift
        // entirely.
        if sig.schema_version != CURRENT_SCHEMA_VERSION {
            return Ok(VerifyOutcome::WarnSchemaMismatch {
                expected: CURRENT_SCHEMA_VERSION,
                found: sig.schema_version,
            });
        }
        if sig_minor != cur_minor {
            return Ok(VerifyOutcome::WarnQdrantMinor {
                expected: CURRENT_QDRANT_VERSION.to_string(),
                found: sig.qdrant_version.clone(),
            });
        }
        Ok(VerifyOutcome::Ok)
    }
}

fn parse_semver_major_minor(s: &str) -> Option<(u32, u32)> {
    // ROBUSTNESS (Gemini PR #2 review, snapshot.rs:231): tolerate the full
    // set of strings Qdrant has ever emitted for its version: bare two-part
    // "1.18", three-part "1.18.0", and pre-release tags like
    // "1.18.0-rc1" / "1.18.0+meta" — the previous split-on-'.' + parse
    // chain rejected all but the canonical three-part shape, which would
    // hard-fail snapshot verification mid-export every time we shipped a
    // pre-release Qdrant during testing.
    //
    // We don't take a direct `semver` crate dep because the rest of the
    // codebase doesn't need full semver, and the existing transitive
    // `semver` (via cargo metadata) isn't guaranteed to stay across deps
    // upgrades. A tiny hand-rolled parser is more robust + zero-cost.
    let trimmed = s.split(|c: char| c == '-' || c == '+').next().unwrap_or(s);
    let mut it = trimmed.split('.');
    let major = it.next()?.trim().parse::<u32>().ok()?;
    let minor_str = it.next().unwrap_or("0").trim();
    let minor = minor_str.parse::<u32>().ok()?;
    Some((major, minor))
}

#[cfg(test)]
mod path_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_sb() -> (TempDir, SnapshotSandbox) {
        let td = TempDir::new().unwrap();
        let root = td.path().to_path_buf().canonicalize().unwrap();
        (td, SnapshotSandbox::with_root(root))
    }

    #[test]
    fn t_export_valid_in_sandbox() {
        let (td, sb) = make_sb();
        let p = td.path().join("memex-export.snapshot");
        let ok = sb.validate_path(&p, SnapshotOp::Export).unwrap();
        assert!(ok.starts_with(td.path().canonicalize().unwrap()));
    }

    #[test]
    fn t_export_outside_sandbox() {
        let (_td, sb) = make_sb();
        let outside = TempDir::new().unwrap();
        let p = outside.path().join("foo.snapshot");
        assert!(sb.validate_path(&p, SnapshotOp::Export).is_err());
    }

    #[test]
    fn t_export_wrong_extension() {
        let (td, sb) = make_sb();
        let p = td.path().join("memex.txt");
        assert!(sb.validate_path(&p, SnapshotOp::Export).is_err());
    }

    #[test]
    fn t_import_valid() {
        let (td, sb) = make_sb();
        let p = td.path().join("existing.snapshot");
        fs::write(&p, b"blob").unwrap();
        assert!(sb.validate_path(&p, SnapshotOp::Import).is_ok());
    }

    #[test]
    fn t_import_nonexistent() {
        let (td, sb) = make_sb();
        let p = td.path().join("nope.snapshot");
        assert!(sb.validate_path(&p, SnapshotOp::Import).is_err());
    }

    #[test]
    fn t_export_overwrites_existing() {
        let (td, sb) = make_sb();
        let p = td.path().join("dup.snapshot");
        fs::write(&p, b"x").unwrap();
        assert!(sb.validate_path(&p, SnapshotOp::Export).is_err());
    }

    #[test]
    fn t_traversal_in_filename() {
        let (td, sb) = make_sb();
        let p = td.path().join("subdir/../../escape.snapshot");
        assert!(sb.validate_path(&p, SnapshotOp::Export).is_err());
    }
}

#[cfg(test)]
mod envelope_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_blob(dir: &Path, name: &str, content: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn t_sign_then_verify_ok() {
        let td = TempDir::new().unwrap();
        let p = write_blob(td.path(), "blob.snapshot", b"hello world");
        SignedEnvelope::sign(&p).unwrap();
        assert!(matches!(
            SignedEnvelope::verify(&p).unwrap(),
            VerifyOutcome::Ok
        ));
    }

    #[test]
    fn t_verify_tampered_blob() {
        let td = TempDir::new().unwrap();
        let p = write_blob(td.path(), "blob.snapshot", b"hello world");
        SignedEnvelope::sign(&p).unwrap();
        fs::write(&p, b"tampered!!").unwrap(); // change blob, sig stale
        assert!(SignedEnvelope::verify(&p).is_err());
    }

    #[test]
    fn t_verify_tampered_sig_sha() {
        let td = TempDir::new().unwrap();
        let p = write_blob(td.path(), "blob.snapshot", b"hello");
        SignedEnvelope::sign(&p).unwrap();
        let sig_path = SignedEnvelope::sig_path_for(&p);
        let mut sig: Signature =
            serde_json::from_slice(&fs::read(&sig_path).unwrap()).unwrap();
        sig.sha256 = "0".repeat(64); // bogus
        fs::write(&sig_path, serde_json::to_vec(&sig).unwrap()).unwrap();
        assert!(SignedEnvelope::verify(&p).is_err());
    }

    #[test]
    fn t_verify_missing_sig_is_legacy() {
        let td = TempDir::new().unwrap();
        let p = write_blob(td.path(), "blob.snapshot", b"hello");
        // no sign() call → legacy
        assert!(matches!(
            SignedEnvelope::verify(&p).unwrap(),
            VerifyOutcome::LegacyNoSignature
        ));
    }

    #[test]
    fn t_verify_schema_mismatch_warns() {
        let td = TempDir::new().unwrap();
        let p = write_blob(td.path(), "blob.snapshot", b"hello");
        SignedEnvelope::sign(&p).unwrap();
        let sig_path = SignedEnvelope::sig_path_for(&p);
        let mut sig: Signature =
            serde_json::from_slice(&fs::read(&sig_path).unwrap()).unwrap();
        sig.schema_version = 1; // current is 3
        fs::write(&sig_path, serde_json::to_vec(&sig).unwrap()).unwrap();
        // need to rewrite sha256 so the blob check passes
        let bytes = fs::read(&p).unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        let sha = to_hex(&h.finalize());
        sig.sha256 = sha;
        fs::write(&sig_path, serde_json::to_vec(&sig).unwrap()).unwrap();
        let outcome = SignedEnvelope::verify(&p).unwrap();
        assert!(matches!(outcome, VerifyOutcome::WarnSchemaMismatch { .. }));
    }

    #[test]
    fn t_verify_qdrant_major_mismatch_errors() {
        let td = TempDir::new().unwrap();
        let p = write_blob(td.path(), "blob.snapshot", b"hello");
        SignedEnvelope::sign(&p).unwrap();
        let sig_path = SignedEnvelope::sig_path_for(&p);
        let mut sig: Signature =
            serde_json::from_slice(&fs::read(&sig_path).unwrap()).unwrap();
        sig.qdrant_version = "2.0.0".into();
        // recompute sha so blob check passes — we want to verify the version check rejects
        let bytes = fs::read(&p).unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        sig.sha256 = to_hex(&h.finalize());
        fs::write(&sig_path, serde_json::to_vec(&sig).unwrap()).unwrap();
        assert!(SignedEnvelope::verify(&p).is_err());
    }

    #[test]
    fn t_verify_qdrant_minor_mismatch_warns() {
        let td = TempDir::new().unwrap();
        let p = write_blob(td.path(), "blob.snapshot", b"hello");
        SignedEnvelope::sign(&p).unwrap();
        let sig_path = SignedEnvelope::sig_path_for(&p);
        let mut sig: Signature =
            serde_json::from_slice(&fs::read(&sig_path).unwrap()).unwrap();
        sig.qdrant_version = "1.17.0".into();
        let bytes = fs::read(&p).unwrap();
        let mut h = Sha256::new();
        h.update(&bytes);
        sig.sha256 = to_hex(&h.finalize());
        fs::write(&sig_path, serde_json::to_vec(&sig).unwrap()).unwrap();
        let outcome = SignedEnvelope::verify(&p).unwrap();
        assert!(matches!(outcome, VerifyOutcome::WarnQdrantMinor { .. }));
    }

    #[test]
    fn t_envelope_json_malformed() {
        let td = TempDir::new().unwrap();
        let p = write_blob(td.path(), "blob.snapshot", b"hello");
        fs::write(SignedEnvelope::sig_path_for(&p), b"not json").unwrap();
        assert!(SignedEnvelope::verify(&p).is_err());
    }

    #[test]
    fn t_sign_verify_roundtrip_arbitrary_bytes() {
        let td = TempDir::new().unwrap();
        for content in [
            vec![],
            b"x".to_vec(),
            (0..=255u8).collect(),
            b"large blob ".repeat(10_000),
        ] {
            let p = write_blob(td.path(), "rt.snapshot", &content);
            SignedEnvelope::sign(&p).unwrap();
            assert!(matches!(
                SignedEnvelope::verify(&p).unwrap(),
                VerifyOutcome::Ok
            ));
            let _ = std::fs::remove_file(SignedEnvelope::sig_path_for(&p));
            let _ = std::fs::remove_file(&p);
        }
    }
}
