# Upstream PR Candidate 02 — P1 Security Backport

**Source commit (fork):** `f55d417` on `origin/main` (with review-driven hardening from `769af65`)
**Target:** `upstream/main` @ `4973a91`
**Author:** ComBba <cent@naver.com>
**Date analyzed:** 2026-05-19
**Verdict:** **Slice into 3 PRs**, land in order. KF-01 (path sandbox) is the highest-value, smallest-surface candidate and should ship first as a standalone PR. KF-02 + KF-03 are tightly coupled to each other but independent of KF-01 and can ship as one follow-up PR. KH-01 (multi-agent / Codex) is **not viable upstream** as a backport because upstream has no Codex scanner/parser to wire it into.

---

## 1. Inventory of f55d417

Stats:

```
src-tauri/Cargo.lock                    |   2 +
src-tauri/Cargo.toml                    |   2 +     # +sha2 (runtime), +tempfile (dev)
src-tauri/src/commands.rs               |  43 +-    # wired 3 IPC entry points
src-tauri/src/indexer.rs                |   8 +-    # wired predict_next_actions
src-tauri/src/lib.rs                    |   2 +     # pub mod sec; pub mod snapshot;
src-tauri/src/sec.rs                    | 279 ++    # NEW: SandboxRoot, SourceAgent
src-tauri/src/snapshot.rs               | 439 ++    # NEW: SnapshotSandbox + SignedEnvelope
src-tauri/tests/sec_integration.rs      |  30 ++    # NEW
src-tauri/tests/snapshot_integration.rs |  30 ++    # NEW
9 files changed, 829 insertions(+), 6 deletions(-)
```

### Symbols added

| File | Symbols added (public) | Symbols added (private) |
|---|---|---|
| `sec.rs` | `SourceAgent` (enum: `ClaudeCode`, `Codex`), `SourceAgent::as_str`, `SandboxRoot`, `SandboxRoot::from_env`, `SandboxRoot::contains`, `SandboxRoot::detect_agent`, `SandboxRoot::roots`, `validate_session_path` | `#[cfg(test)] SandboxRoot::from_roots` |
| `snapshot.rs` | `SNAPSHOT_EXT`, `SIG_EXT`, `CURRENT_SCHEMA_VERSION` (3), `CURRENT_QDRANT_VERSION` ("1.18.0"), `ISSUER`, `SnapshotOp` (enum: `Export`, `Import`), `SnapshotSandbox`, `SnapshotSandbox::from_env`, `SnapshotSandbox::with_root` (doc-hidden), `SnapshotSandbox::validate_path`, `SnapshotSandbox::root`, `Signature` struct, `VerifyOutcome` (4 variants), `SignedEnvelope`, `SignedEnvelope::sig_path_for`, `SignedEnvelope::sign`, `SignedEnvelope::verify` | `parse_semver_major_minor` |
| `lib.rs` | `pub mod sec;`, `pub mod snapshot;` | — |

### Call-site wiring (in `commands.rs` + `indexer.rs`)

1. `commands::get_session_turns` (line 192-195): `validate_session_path` before `parse_session`. **Input source: Qdrant payload (tainted).**
2. `commands::snapshot_export` (line 247): `SnapshotSandbox::validate_path(_, Export)` → `indexer::snapshot_export` → `SignedEnvelope::sign`.
3. `commands::snapshot_import` (line 252-288): `SnapshotSandbox::validate_path(_, Import)` → `SignedEnvelope::verify` → `indexer::snapshot_import`. **Return type widened `Result<(), String>` → `Result<String, String>`** (audit MED-1) so the frontend can surface legacy/schema/version warnings.
4. `indexer::predict_next_actions` (line 1267): validate active session_path. **Input source: Qdrant payload.**
5. `indexer::predict_next_actions` (line 1340-1343): validate each neighbour `source_path`. **Input source: Qdrant payload.**

### Review-driven hardening landed later (commit 769af65) — should be folded into any upstream PR

- `sec.rs`: `std::env::var_os("HOME")` → `dirs::home_dir()` (Windows compat).
- `snapshot.rs::SnapshotSandbox::from_env`: hardcoded `~/Library/Application Support/...` → `dirs::data_dir().join("dev.sgwannabe.memex/snapshots")` (cross-platform).
- `snapshot.rs::SnapshotSandbox::validate_path` (Export branch): if the target path already exists as an in-sandbox symlink to an out-of-sandbox file, the previous logic followed it silently. Now canonicalizes the full path and rejects if it escapes (closes a known symlink-escape vector).
- `snapshot.rs::SignedEnvelope::verify`: schema-mismatch check now runs **before** qdrant-minor-mismatch (schema is higher-risk; previously a concurrent schema drift was hidden).
- `snapshot.rs::parse_semver_major_minor`: tolerates `1.18`, `1.18.0`, `1.18.0-rc1`, `1.18.0+meta`.
- `commands.rs::snapshot_export`: atomicity — if `SignedEnvelope::sign` fails, delete the unsigned snapshot before returning, so the next export isn't blocked by the leftover.
- New dep: `dirs = "5"` (runtime).

These hardenings are non-optional for an upstream PR — upstream maintainers will demand them and CodeRabbit/Gemini already caught them on the fork.

---

## 2. Per-Component Feasibility

### KF-01 — Path Sandbox (sec.rs) — **score 4/5 (recommend)**

**What it does.** Adds a single Rust module (`sec.rs`) that, given a path read from Qdrant payload, canonicalizes it and confirms it lives under one of the allowed roots (`~/.claude/projects` for upstream; the fork adds `~/.codex/sessions`). Rejects NUL bytes, empty strings, symlink-escape, and `..` traversal. Wraps in a one-shot `validate_session_path()` helper for IPC entry points.

**Fork-only dependencies it carries.**

- `SourceAgent::Codex` variant + `~/.codex/sessions` root. **Easy to strip** for an upstream PR — drop the `Codex` enum variant, drop the second `candidates` entry, simplify the error message. The `SourceAgent` enum can stay as a single-variant for future-proofing OR be removed entirely (upstream has no use for it today). Recommend: keep `SourceAgent::ClaudeCode` only, leave the enum as the extension point.
- No other fork-only types. `sec.rs` does not depend on `enrich.rs`, `memex_sessions_v3`, `crud.rs`, or any P2+ types.

**Standalone-against-upstream?** Yes. The `sec.rs` module compiles standalone on top of `upstream/main` with the single change of dropping the Codex variant (or keeping it dormant). The `Cargo.toml` adds (`sha2`, `tempfile`, `dirs`) are needed only if you also ship KF-02/KF-03. For KF-01 alone, only `dirs` (runtime) and `tempfile` (dev) are needed.

**Minimum diff against upstream/main:** Detailed in section 3 below.

---

### KF-02 — Snapshot Sandbox (snapshot.rs, `SnapshotSandbox` half) — **score 4/5**

**What it does.** A second sandbox specifically for snapshot files: enforces that the snapshot path's parent directory is under `~/Library/Application Support/dev.sgwannabe.memex/snapshots` (or the platform equivalent via `dirs::data_dir()`), the file has the `.snapshot` extension, no `..` segments, no NUL bytes. Distinguishes Export (refuses overwrite) from Import (refuses missing file).

**Fork-only dependencies.** **None.** Pure path-handling code over `dirs` + `std::fs`. The Tauri bundle identifier `dev.sgwannabe.memex` is the same on upstream (`src-tauri/tauri.conf.json`).

**Standalone-against-upstream?** Yes, but with one wrinkle: the existing upstream `snapshot_export_default` command writes to `$HOME/memex-snapshot-<ts>.snapshot` (i.e. **outside the proposed sandbox**). Shipping KF-02 standalone would either (a) break `snapshot_export_default` (the user's home dir is no longer accepted) or (b) require also migrating `snapshot_export_default` to write into the sandbox dir. Recommend (b) — same PR.

**Minimum diff:** `snapshot.rs` (sandbox half only, ~140 lines + tests) + `commands.rs::snapshot_export/import/export_default` wiring + `Cargo.toml`: add `dirs`. No `sha2` needed if KF-03 ships separately.

---

### KF-03 — Signed Envelope (snapshot.rs, `SignedEnvelope` half) — **score 4/5**

**What it does.** Sidecar SHA-256 signature for snapshots, written as `<file>.snapshot.sig`. On import, verifies SHA-256 matches; checks `schema_version` and `qdrant_version` against compile-time constants; returns `VerifyOutcome::{Ok, LegacyNoSignature, WarnSchemaMismatch, WarnQdrantMinor}` for non-fatal cases, `Err` for tamper / qdrant major mismatch / malformed sig. **Critically: backwards compatible** — snapshots without a `.sig` sidecar are accepted as legacy (with a warning), so existing users' snapshot files keep working.

**Fork-only dependencies.** **None.** Only depends on `sha2` (new), `chrono` (already in upstream), `serde`/`serde_json` (already in upstream).

**Standalone-against-upstream?** Yes — the sidecar approach is specifically designed to not touch `indexer::snapshot_export/import`, so the existing snapshot HTTP-to-Qdrant flow is untouched. Wiring requires:
- `commands.rs::snapshot_export` to call `SignedEnvelope::sign` after `indexer::snapshot_export`.
- `commands.rs::snapshot_import` to call `SignedEnvelope::verify` before `indexer::snapshot_import`.
- API breaking change: `snapshot_import` return type widens `Result<(), String>` → `Result<String, String>` (the warning text). Upstream frontend would need a 1-line update (or upstream keeps `Result<(), String>` and just logs warnings server-side — defensible choice).

**Tight coupling with KF-02.** KF-03 alone (without KF-02's sandbox) is technically possible — you can sign any path the user supplies — but the threat model is weaker: an attacker who can write to the snapshot path can write to its `.sig` sidecar too. KF-02 + KF-03 together give the full "tamper-evident, sandbox-confined" guarantee. **Recommend bundling them.**

---

### KH-01 — Multi-Agent (Codex CLI support) — **score 0/5 (do NOT propose upstream)**

**What it does.** Lets Memex index Codex CLI sessions from `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` in addition to Claude Code sessions. In `f55d417` itself this is **only** the `SourceAgent::Codex` enum variant and the second sandbox root in `sec.rs::SandboxRoot::from_env`. The actual Codex parser, scanner, and per-payload `source_agent` tagging live in **later P2-P7 commits** (not in `f55d417`).

**Fork-only dependencies.** Everything. The Codex parser, the v3 collection schema with `source_agent` payload field, the dual-source scanner — none of it exists in upstream. Shipping `SourceAgent::Codex` to upstream alone would add a dead enum variant.

**Standalone-against-upstream?** No. The variant is a forward-looking declaration with no consumer upstream. Either:
- Strip it entirely from the upstream PR (recommended — KF-01 ships without it).
- Or ship the entire fork branch (Codex parser, v3 schema, scanner — multiple thousand lines, definitively not a "small PR"). That's its own multi-PR effort, **separate from the security work**.

**Verdict.** **Do not include in the security backport.** If the fork wants Codex upstream, it's a separate, much larger feature proposal.

---

## 3. KF-01 Deep-Dive — The Recommended First PR

This is the lowest-risk, highest-value slice. It closes a real arbitrary-file-read vulnerability that exists today on `upstream/main` (any process that can write to the Qdrant payload — e.g. a malicious snapshot import, or a Qdrant compromise — can make Memex parse arbitrary files via `parse_session`).

### Exact diff against upstream/main

```diff
diff --git a/src-tauri/Cargo.toml b/src-tauri/Cargo.toml
--- a/src-tauri/Cargo.toml
+++ b/src-tauri/Cargo.toml
@@ -37,8 +37,12 @@
 futures = "0.3"
 petgraph = "0.6"
+# Cross-platform home directory resolution. Used by the path sandbox so the
+# same code resolves `~` on Windows (where $HOME is conventionally absent).
+dirs = "5"

 [dev-dependencies]
 pretty_assertions = "1"
+tempfile = "3"
```

```diff
diff --git a/src-tauri/src/lib.rs b/src-tauri/src/lib.rs
--- a/src-tauri/src/lib.rs
+++ b/src-tauri/src/lib.rs
@@ -2,6 +2,7 @@ pub mod cli;
 pub mod commands;
 pub mod indexer;
 pub mod mcp;
 pub mod parser;
+pub mod sec;
 pub mod watcher;
```

**New file: `src-tauri/src/sec.rs`** — adapted from the fork's version (origin/main, 284 lines), with the `Codex` variant dropped. Approximate net size: ~210 lines including tests.

The simplified `SandboxRoot::from_env` for upstream:

```rust
pub fn from_env() -> Result<Self> {
    let home = dirs::home_dir().context("could not resolve home directory")?;
    let claude_root = home.join(".claude/projects");
    let canonical = claude_root
        .canonicalize()
        .with_context(|| format!("canonicalize {}", claude_root.display()))?;
    Ok(Self { canonical_root: canonical })
}
```

All other methods (`contains`, `validate_session_path`, NUL/symlink/traversal handling) lift verbatim. The 14-test unit suite ports over directly; just drop the Codex-specific tests (`t_valid_codex_session_path`, `t_graceful_codex_missing`).

```diff
diff --git a/src-tauri/src/commands.rs b/src-tauri/src/commands.rs
--- a/src-tauri/src/commands.rs
+++ b/src-tauri/src/commands.rs
@@ -192,8 +192,9 @@ pub async fn get_session_turns(
         .ok_or_else(|| "session payload missing source_path".to_string())?;
-    let session = parser::parse_session(std::path::Path::new(&source))
+    let validated = crate::sec::validate_session_path(std::path::Path::new(&source))
         .map_err(stringify)?;
+    let session = parser::parse_session(&validated).map_err(stringify)?;
     serde_json::to_value(&session).map_err(stringify)
 }
```

```diff
diff --git a/src-tauri/src/indexer.rs b/src-tauri/src/indexer.rs
--- a/src-tauri/src/indexer.rs
+++ b/src-tauri/src/indexer.rs
@@ -1264,7 +1264,8 @@ pub async fn predict_next_actions(
         .context("active session payload is missing source_path")?;
-    let active = crate::parser::parse_session(StdPath::new(&source_path))?;
+    let validated = crate::sec::validate_session_path(StdPath::new(&source_path))?;
+    let active = crate::parser::parse_session(&validated)?;
     if active.turns.is_empty() {
@@ -1338,7 +1339,10 @@ pub async fn predict_next_actions(
     for (nb_sid, sim_score, source, nb_project) in &neighbor_meta {
-        let Ok(nb) = crate::parser::parse_session(StdPath::new(source)) else { continue };
+        // SAFETY: neighbor source_path came from Qdrant payload — validate
+        // it lives inside the allowed sandbox root before parsing the JSONL.
+        let Ok(validated) = crate::sec::validate_session_path(StdPath::new(source)) else { continue };
+        let Ok(nb) = crate::parser::parse_session(&validated) else { continue };
         if nb.turns.is_empty() {
             continue;
         }
```

### Call sites in upstream that need to route through the sandbox

Confirmed by `grep -n "parse_session\|source_path"` on `upstream/main`:

| File | Line | Function | Path source | Sandbox needed? |
|---|---|---|---|---|
| `commands.rs` | 195 | `get_session_turns` | Qdrant payload | **YES** (IPC + tainted) |
| `commands.rs` | 580 | `tail_recent_errors` (inner loop) | `WalkDir` rooted at `path.unwrap_or_else(default_projects_root)` | **YES if `path` is IPC-supplied** — `WalkDir` paths are bounded by the root, but the root itself comes from IPC. Recommend: sandbox the *root* before walking. |
| `commands.rs` | 605 | `tail_recent_errors` (`source_path` synthesis) | derived from same `WalkDir` | covered transitively |
| `indexer.rs` | 1267 | `predict_next_actions` (active) | Qdrant payload | **YES** |
| `indexer.rs` | 1341 | `predict_next_actions` (neighbour loop) | Qdrant payload | **YES** |

**Recommendation for the first PR:** wire the 3 hot, definitely-tainted call sites (`get_session_turns`, `predict_next_actions ×2`). `tail_recent_errors` is borderline — its `path` parameter is IPC-tainted, but the function constrains the walk to that root. The minimal hardening is to validate the root parameter against the sandbox once at function entry. Adding that fix to the same PR is ~3 lines and worth the consistency.

### Backwards-compat notes

- **Default-config users (`~/.claude/projects` exists, no IPC-supplied paths):** no behavior change. `validate_session_path` succeeds on every legitimate path.
- **Users without `~/.claude/projects`:** `SandboxRoot::from_env` will return `Err`. Upstream today already calls `default_projects_root()` and tries to walk it; if that dir doesn't exist, upstream returns an empty result. The new behavior would surface a clearer error (`"no valid session root found"`). **This is arguably an improvement** but is a small surface change — call it out in the PR description.
- **Symlinked session directories (rare but exists):** a user who symlinks `~/.claude/projects` to e.g. `/Volumes/External/claude-projects` is fine — `canonicalize` resolves the link once at sandbox construction time, and `contains` canonicalizes the candidate path against the canonical root. The fork's `t_symlink_inside` test covers this.
- **Tauri command return types:** **NO API changes for the KF-01-only PR.** All four wired functions keep the same signature; only behavior on adversarial input changes (now: reject; before: read arbitrary file).

### Cargo dependency additions (KF-01 only)

- `dirs = "5"` (runtime, ~10 transitive crates, all stable, MIT/Apache-2.0).
- `tempfile = "3"` (dev-dependencies, for tests).

Neither pulls anything controversial. `dirs` is by `xdg-base-dir` author and used across the Rust ecosystem (rustup, cargo, etc.).

---

## 4. Threat Model + Testing

### What each component prevents

| KICK | Attack | Pre-fix outcome | Post-fix outcome |
|---|---|---|---|
| **KF-01** | Attacker writes a Qdrant payload with `source_path = "/etc/shadow"` (e.g. via compromised snapshot, or Qdrant network-exposed without auth, or malicious MCP server upserting points). Memex IPC `get_session_turns` or `predict_next_actions` then reads `/etc/shadow` and either parses it as JSONL (failing silently) or returns chunks of it to the renderer/frontend. | Arbitrary file read inside Memex's user-data permissions (high — includes `~/.ssh`, browser cookies, etc.). | Rejected at the IPC boundary with "path outside sandbox". |
| **KF-01** | Attacker uses a symlink already inside `~/.claude/projects` pointing to `/etc/shadow`. (Requires the attacker to already have user write access — but in that case symlink-planting is the cheaper escalation.) | Same arbitrary file read. | Canonicalization resolves the symlink target; rejection if target is outside sandbox. |
| **KF-01** | Path-traversal: `~/.claude/projects/foo/../../../etc/shadow` slipped through string concatenation upstream of `parse_session`. | Read of `/etc/shadow`. | `canonicalize()` normalizes the `..`; sandbox check then fails. |
| **KF-02** | Attacker hands `snapshot_import` a path to `/etc/shadow` or to a remote-mounted volume. Or: socially-engineered `snapshot_export` writes to `/etc/passwd`. | Read/write of arbitrary file as Memex user. | Path-outside-sandbox rejection; wrong-extension rejection. |
| **KF-02** | Attacker plants a symlink inside `~/Library/Application Support/dev.sgwannabe.memex/snapshots/foo.snapshot` → `/etc/shadow`, then triggers Export (overwrite-refused) or Import. | Read of target. | Post-fix `769af65` hardening: full-path `canonicalize` on Export catches in-sandbox-symlink-to-outside. |
| **KF-03** | Attacker swaps a benign exported snapshot with a malicious one that, when imported into Qdrant, corrupts the collection or exfiltrates via a deliberately-crafted payload. Or: tampers with the snapshot byte-stream to alter point counts/vectors. | Silent tampering accepted; Qdrant collection contents corrupted. | SHA-256 mismatch → import rejected with `Err`. |
| **KF-03** | Attacker imports a snapshot from a future Qdrant major version (e.g. 2.0.0) whose binary format is incompatible. | Qdrant import succeeds-then-crashes-or-corrupts at query time. | Qdrant major-version mismatch → import rejected with `Err`. |
| **KF-03** | Schema drift (snapshot generated by an older Memex with `schema_version=1`, current is `3`). | Silent partial-load: payload field names differ, queries return wrong results. | `WarnSchemaMismatch` surfaced to user via return value; import proceeds (warn, don't break legacy users). |
| **KF-03** | Legacy snapshot (no `.sig` sidecar) imported on a new Memex. | Unsigned import treated as fully trusted. | `LegacyNoSignature` warning surfaced; import proceeds (backwards compat). |

### Testing matrix

| Layer | Existing fork coverage | What an upstream PR needs |
|---|---|---|
| **Unit (KF-01)** | 14 tests in `sec.rs`: valid Claude path, valid Codex path, `/etc/passwd` reject, outside-tmp reject, `..` traversal reject, symlink-outside reject, symlink-inside accept, NUL byte reject, empty string reject, nonexistent path reject, canonical idempotent, graceful Codex-missing, `SourceAgent::as_str`, no-panic on arbitrary bytes | Drop the 2 Codex-specific tests; keep the other 12. Add a `#[cfg(windows)]` test for `dirs::home_dir()` returning `%USERPROFILE%` if Windows CI is in scope (likely defer). |
| **Unit (KF-02)** | 7 tests in `snapshot.rs::path_tests`: valid export, outside reject, wrong extension reject, valid import, nonexistent import reject, overwrite reject, traversal-in-filename reject | Lift verbatim. |
| **Unit (KF-03)** | 9 tests in `snapshot.rs::envelope_tests`: sign-then-verify, tampered blob, tampered sig sha, missing sig (legacy), schema mismatch warn, qdrant major mismatch err, qdrant minor mismatch warn, malformed JSON, arbitrary-bytes roundtrip | Lift verbatim. Add a regression test for `parse_semver_major_minor("1.18.0-rc1")` from the 769af65 hardening. |
| **Integration** | 4 tests in `tests/sec_integration.rs` + `tests/snapshot_integration.rs`: `it_sandbox_from_env_succeeds_if_any_root_exists`, `it_rejects_etc_passwd`, `it_sign_then_verify_in_sandbox`, `it_legacy_snapshot_is_warned_not_rejected` | Lift verbatim; the Codex check in `it_sandbox_from_env_succeeds_if_any_root_exists` becomes Claude-only. |
| **Manual smoke** | (covered by fork's E2E suite, PR #10) | (1) Index a real Claude project, hit `get_session_turns` from the dashboard — expect no behavior change. (2) Inject `source_path: "/etc/passwd"` into a Qdrant payload, retry — expect rejection. (3) Try `snapshot_export("/tmp/foo.snapshot")` — expect rejection. (4) Export+import a snapshot through the dashboard — expect success with `Ok` outcome. (5) Tamper with a snapshot file by `echo X >> foo.snapshot`, retry import — expect rejection. |

### Negative-test gaps worth adding for upstream

- **CVE-2022-21658 race** (`canonicalize` then act): the fork's design `validate → return canonical → use canonical` is correct (use the canonical, not the original). Worth an inline comment in the PR.
- **macOS HFS+ case folding**: `~/.CLAUDE/projects` would canonicalize to `~/.claude/projects` on case-insensitive HFS+. Probably benign but worth a test on macOS CI.

---

## 5. Recommended PR Shape

### Three PRs, landed in order

| PR | Scope | Size | Risk | Why this order |
|---|---|---|---|---|
| **PR-A: KF-01 path sandbox** | `sec.rs` (Claude-only), `lib.rs`, wiring in 3-4 IPC entry points, `Cargo.toml` (+`dirs`, dev `tempfile`) | ~330 lines (210 module + 100 tests + 20 wiring) | Low — additive module, no API breakage, opt-in error on adversarial input only | Standalone-shippable, closes the broadest attack surface (arbitrary file read via Qdrant payload), and prepares the codebase to accept KF-02/KF-03 without conflict. |
| **PR-B: KF-02 + KF-03 (snapshot sandbox + envelope)** | `snapshot.rs`, wiring in `commands::snapshot_export`/`snapshot_import`/`snapshot_export_default`, `Cargo.toml` (+`sha2`) | ~520 lines (440 module + 60 tests + 20 wiring) | Medium — `snapshot_import` return type widens from `Result<(), String>` to `Result<String, String>`, requires a 1-line frontend update; `snapshot_export_default` semantics change (file lands in sandbox dir, not `$HOME`) | Sandbox + envelope are tightly coupled in their threat model. Shipping envelope without sandbox is weaker (attacker who can write the snapshot can write the sig). Land after PR-A so reviewers see the pattern. |
| **PR-C (optional, not recommended now): Codex multi-agent** | Codex parser, scanner, payload tagging, v3 collection schema. **Multi-thousand-line feature**, not a security PR. | not estimated here | High — full new feature surface | Separate proposal entirely. Belongs in its own design discussion with upstream maintainers, not in the security thread. |

### Why not one big PR

- The fork's `f55d417` is 829 lines + later hardening. Upstream reviewers will be slower to merge a 1000+-line "security feature" PR than two focused 300-500-line PRs.
- KF-01 has zero API surface change. KF-02/KF-03 change `snapshot_import`'s return type and `snapshot_export_default`'s output location — that's a meaningful behavior delta deserving its own discussion thread.
- If KF-02/KF-03 stall in review (e.g. maintainer wants a different signing scheme like Ed25519 instead of bare SHA-256, or a different sandbox layout), KF-01's value still lands.

### Why KF-01 first, not KF-03

KF-01 is the textbook small security fix: clear bug (arbitrary file read), clear fix (path canonicalization + allowlist), no API change, well-tested. KF-03 is more design-flavored (sidecar vs embedded sig, SHA-256 vs Ed25519, version-compat policy) and will provoke more bike-shedding.

### Caveat — items that won't ship cleanly even sliced

1. **`SourceAgent::Codex` and the dual-root design.** Strip from KF-01 PR. The enum can stay as `enum SourceAgent { ClaudeCode }` to mark it as an extension point, but more honest to omit entirely and let upstream add it back when/if Codex parsing lands.
2. **`CURRENT_QDRANT_VERSION = "1.18.0"` constant.** This is fork-specific (the fork pins to 1.18). Upstream uses qdrant-client 1.x but doesn't pin a server version. For the upstream PR, recommend either (a) make it a configurable constant read from `Cargo.toml` `[package.metadata]`, or (b) document the choice and accept that upstream will adjust on each Qdrant bump.
3. **`schema_version = 3`.** The fork's `3` reflects the v3 collection schema (`memex_sessions_v3`, KG-03). Upstream is on the v2 schema. **For the upstream PR, set `CURRENT_SCHEMA_VERSION = 2`.** That's a one-character change and an honest reflection of the upstream state.
4. **The fork's `tail_recent_errors` is NOT wired to the sandbox in `f55d417`.** This is a gap in the fork; if you ship KF-01 upstream, include the `tail_recent_errors` root-validation as a 3-line addition (same PR, called out separately).

---

## Appendix — Files referenced

All paths absolute on this machine.

- Fork source (post-hardening): `/Users/kimsejun/Documents/GitHub/memex/src-tauri/src/sec.rs`
- Fork source (post-hardening): `/Users/kimsejun/Documents/GitHub/memex/src-tauri/src/snapshot.rs`
- Fork tests: `/Users/kimsejun/Documents/GitHub/memex/src-tauri/tests/sec_integration.rs`
- Fork tests: `/Users/kimsejun/Documents/GitHub/memex/src-tauri/tests/snapshot_integration.rs`
- Upstream baseline: `git show upstream/main:src-tauri/src/commands.rs` (lines 192-195, 247-266, 535-580)
- Upstream baseline: `git show upstream/main:src-tauri/src/indexer.rs` (lines 1244-1345, 1527-1580)
- Upstream Cargo.toml: `git show upstream/main:src-tauri/Cargo.toml`
- Original fork commit: `git show f55d417` (829 lines)
- Hardening follow-up: `git show 769af65 -- src-tauri/src/sec.rs src-tauri/src/snapshot.rs src-tauri/src/commands.rs`

This analysis was performed against `upstream/main` at `4973a91` and `origin/main` at `8509096` (fetched 2026-05-19).
