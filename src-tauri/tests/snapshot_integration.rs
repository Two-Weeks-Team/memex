//! Integration: snapshot envelope sign/verify flow.

use memex_lib::snapshot::{SignedEnvelope, SnapshotOp, SnapshotSandbox, VerifyOutcome};
use std::fs;
use tempfile::TempDir;

#[test]
fn it_sign_then_verify_in_sandbox() {
    let td = TempDir::new().unwrap();
    let sb = SnapshotSandbox::with_root(td.path().canonicalize().unwrap());
    let p = td.path().join("memex-test.snapshot");
    fs::write(&p, b"hello").unwrap();
    let canonical = sb.validate_path(&p, SnapshotOp::Import).unwrap(); // use Import so existence isn't blocked
    let _ = SignedEnvelope::sign(&canonical).unwrap();
    assert!(matches!(
        SignedEnvelope::verify(&canonical).unwrap(),
        VerifyOutcome::Ok
    ));
}

#[test]
fn it_legacy_snapshot_is_warned_not_rejected() {
    let td = TempDir::new().unwrap();
    let p = td.path().join("legacy.snapshot");
    fs::write(&p, b"legacy").unwrap();
    assert!(matches!(
        SignedEnvelope::verify(&p).unwrap(),
        VerifyOutcome::LegacyNoSignature
    ));
}
