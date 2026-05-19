# Upstream PR Security Review
**Reviewer**: Security Engineer (claude-sonnet-4-6)  
**Date**: 2026-05-19  
**Scope**: fork ComBba/memex → upstream sgwannabe/memex candidate commits  
**Fork HEAD**: `8509096` | **Upstream HEAD**: `4973a91`

---

## Executive Summary

| Commit | Description | Verdict |
|--------|-------------|---------|
| `e402b1f` | mix-modal self-contained picker (UI) | SHIP_WITH_CHANGES |
| `f55d417` | P1 security bundle | SHIP_WITH_CHANGES |
| `2b59dc9` | predict Codex parser routing | SHIP_WITH_CHANGES |
| `deed283` | heat-trail SVG stroke cap | SHIP |
| `e1c075b` | cli ensure v3 collection | SHIP |
| `84db1fc` | enable WebView devtools | BLOCK |

---

## §1 Secret Leakage Scan

Ran: `git show f55d417 e402b1f deed283 2b59dc9 e1c075b 84db1fc | grep -iE "key|secret|token|password|apikey"` plus value-pattern scan for base64/hex strings of ≥16 chars.

**Result: No credentials found.**

- The only `"secret"` match is in `sec.rs` test code (`fs::write(&outside_file, "secret")`) — it is a literal test string written to a temp file to verify sandbox rejection, not a credential.
- All `key:` matches are Qdrant payload field name strings (`"start_ts_dt"`, `"project_name"`, `"tool_count"`).
- `sha2 = "0.10.9"` crate hash in `Cargo.lock` verified — no embedded secrets.
- No `.env` file changes, no hardcoded API keys, no bearer tokens.

**Credential leak risk: NONE** for all six commits.

---

## §2 Per-Candidate OWASP Review

### 2.1 `e402b1f` — mix-modal self-contained picker

**Files**: `src/index.html`, `src/main.js`, `src/styles.css`

| OWASP Category | Finding | Severity |
|----------------|---------|----------|
| A03 Injection | No SQL/command injection surface. All backend calls go through Tauri IPC with typed Rust handlers. | None |
| A03 XSS | **Partially addressed but one gap present** (see below) | Medium |
| A01 Auth | No auth surface; frontend-only UI change. | N/A |
| A02 Sensitive Data | Session metadata (project_name, ai_title) rendered in picker — already in-scope for the app. No new leakage. | None |
| A04 Insecure Design | UUID-direct-pick path: `uuidRe.test(q)` then renders a picker row without a Qdrant round-trip (main.js line ~174). The `session_id` is passed directly to `addToMix("positive", hit.session_id)`. The session_id itself is never rendered as HTML — it is only passed as a string argument to the Rust IPC. Safe. | None |
| A05 Security Misconfiguration | N/A (no config change) | None |
| A09 Logging | N/A | None |

**XSS Detail (Medium)**

`renderMixPickerRow` at `main.js` (post-commit line ~218-221) uses `innerHTML` with template literal:

```javascript
meta.innerHTML = `
  <span class="mix-picker-project">${escapeHtml(project)}</span>
  <span class="mix-picker-title">${escapeHtml(title.slice(0, 80))}</span>
  <span class="mix-picker-start">${escapeHtml(start)}</span>
`;
```

The `escapeHtml` function (`main.js:2600`) correctly escapes `& < > " '`. The three interpolated values (`project`, `title`, `start`) all pass through `escapeHtml` before injection into innerHTML. This is correct.

However: the error-path template literal at line ~197:
```javascript
results.innerHTML = `<p class="mix-picker-empty">Search failed: ${escapeHtml(String(err))}</p>`;
```
This is also correctly escaped. `String(err)` converts the Error object to its message string, which is then passed through `escapeHtml`.

**Gap identified**: The `pickerInput.value = state.query || ""` at modal open (`openMixModal`) sets the INPUT value field from user-generated state. This does not use `innerHTML` — it sets `value` property, which is safe. However, the auto-fire of `runMixPickerSearch()` when `state.query` is non-empty means any Qdrant-returned `project_name` or `ai_title` containing malformed Unicode or multi-byte sequences is truncated by `.slice(0, 80)` without byte-boundary checks. This is a theoretical data corruption edge (not XSS) because `escapeHtml` is applied first and `slice` on already-escaped strings can't produce valid HTML injection.

**Verdict**: XSS handled correctly. No exploitable path found. The medium severity is the pattern risk (innerHTML + template literal) — if a future developer removes `escapeHtml` from one interpolation, it becomes exploitable.

**Recommendation**: Replace the `meta.innerHTML = \`...\`` block with DOM `createElement`/`textContent` assignments to make XSS structurally impossible, not just policy-dependent:

```javascript
// Preferred: DOM construction instead of innerHTML template
const projSpan = document.createElement("span");
projSpan.className = "mix-picker-project";
projSpan.textContent = project;          // XSS impossible
const titleSpan = document.createElement("span");
titleSpan.className = "mix-picker-title";
titleSpan.textContent = title.slice(0, 80);
const startSpan = document.createElement("span");
startSpan.className = "mix-picker-start";
startSpan.textContent = start;
meta.append(projSpan, titleSpan, startSpan);
```

**Verdict: SHIP_WITH_CHANGES** — ship if upstream PR explicitly notes the innerHTML→textContent migration as a follow-up, or apply the fix in the backport rewrite.

---

### 2.2 `f55d417` — P1 security bundle (KF-01/02/03 + KH-01)

**Files**: `src-tauri/src/sec.rs`, `src-tauri/src/snapshot.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/indexer.rs`

This commit is the highest-value security change in the set. The threat model exercise follows in §3.

| OWASP Category | Finding | Severity |
|----------------|---------|----------|
| A01 Auth / A04 Insecure Design | Path sandbox (KF-01) gates `get_session_turns` and `predict_next_actions` before JSONL reads. Correct placement at IPC boundary. | Positive control |
| A03 Injection | Path-to-filesystem injection (Tampered Qdrant payload → arbitrary file read) is now blocked by `validate_session_path` on both active session and all neighbor paths. | Mitigated |
| A02 Sensitive Data | `SignedEnvelope` uses SHA-256 (unauthenticated hash). See §3 for detailed weakness. | Medium |
| A05 Security Misconfiguration | `SnapshotSandbox::from_env()` hardcodes `~/Library/Application Support/dev.sgwannabe.memex/snapshots`. Deferred NIT-1 (derive from `app_data_dir()`) is the correct long-term fix. | Low |
| A09 Logging | `eprintln!("[memex] {msg}")` on legacy/schema/version warnings leaks internal version info to stderr. Acceptable for a developer tool. | Informational |
| A06 Vulnerable Components | `sha2 = "0.10.9"` — no known CVEs. `tempfile = "3"` (dev-only) — no known CVEs. | None |

**Coverage gap (High — pre-existing, not introduced by this commit)**

`tail_recent_errors` (IPC command) accepts an optional `path: Option<PathBuf>` parameter with no sandbox validation (`commands.rs:596-603`). The frontend call at `main.js:3075` always passes `sinceSeconds` with no path, so the path defaults to `default_projects_root()`. But since `withGlobalTauri = true` and `csp: null`, a hypothetical XSS payload or malicious deep-link handler could call:

```javascript
window.__TAURI__.invoke("tail_recent_errors", { path: "/etc" })
```

This would cause Memex to walkdir `/etc`, find no `.jsonl` files, and return an empty array — a directory traversal with no data exfiltration in practice (`.jsonl` extension filter applies). However, the attack surface is open. This pre-existed the commit but `f55d417` is the right place to have fixed it.

Similarly: `list_sessions` accepts `path: Option<PathBuf>` with no sandbox validation.

These are pre-existing issues not introduced by f55d417, but the security commit that adds a sandbox should also gate these commands.

**Verdict: SHIP_WITH_CHANGES** — the core sandbox controls are correct. Two changes required before upstream PR:
1. Gate `tail_recent_errors` and `list_sessions` optional `path` through `SandboxRoot::contains()` or reject the parameter entirely (force default).
2. See §3 for signing scheme advisory.

---

### 2.3 `2b59dc9` — predict Codex parser routing

**Files**: `src-tauri/src/indexer.rs`

| OWASP Category | Finding | Severity |
|----------------|---------|----------|
| A03 Injection | `source_agent` is read from Qdrant payload and compared to the literal string `"codex"`. This is a simple string equality check — not used in any command construction or path operation. Cannot cause injection. | None |
| A01 Auth | The `validate_session_path` call is retained on both active session and neighbor paths (inherited from f55d417). The parser routing fix does not bypass or remove these checks. | None |
| A04 Insecure Design | `source_agent` defaults to `"claude_code"` when absent (legacy v2 data). The default is safe — it routes to the Claude parser which fails gracefully on Codex-format files rather than executing arbitrary code. | None |
| A02 Sensitive Data | The `PREDICT_PARSE_CACHE` (LRU memo keyed by path+mtime) now carries source_agent in the neighbor tuple. The cache key is the canonical validated path — no information from unvalidated user input enters the cache key. | None |
| A06 Vulnerable Components | No new dependencies introduced. | None |

**Observation**: The `source_agent` value comes from Qdrant payload, which itself was written by the fork's own indexer. A tampered Qdrant database could supply `source_agent = "anything_else"` to trigger the `claude_code` default branch. This is benign — the worst outcome is the Claude parser fails on a Codex file and `active.turns.is_empty()` triggers an early return with "no predictions." No code execution path exists through this branch.

**Verdict: SHIP_WITH_CHANGES** — note that this commit depends on `codex_parser.rs` which is a fork-only module. It cannot be cherry-picked to upstream without also shipping P5 (codex_parser). This is a backport dependency constraint, not a security issue per se. If submitted to upstream, the commit requires the codex parser module as a prerequisite.

---

### 2.4 `deed283` — heat-trail SVG stroke cap (cosmetic + score data)

**Files**: `src/main.js`, `src/styles.css`

| OWASP Category | Finding | Severity |
|----------------|---------|----------|
| A03 XSS | SVG is created via `document.createElementNS` + `setAttribute`. No innerHTML used for SVG elements. Score values are passed through `clampUnit()` which coerces to numeric `[0,1]` before use in `setAttribute("stroke-width", String(strokePx))`. `String(numeric)` cannot produce executable content in an SVG attribute. | None |
| A03 Injection | `n.score` from Qdrant lens results is a float. `clampUnit` applies `Number.isFinite` + numeric clamp — defense-in-depth against NaN/Infinity smuggled in a tampered Qdrant response. | Mitigated |
| A02 Sensitive Data | The `score` value used for visual rendering is the raw fused similarity score. This is already displayed in the breakdown panel by other code — no new information exposure. | None |
| A04 Insecure Design | `preserveAspectRatio="none"` change prevents the viewBox scaling amplification that produced the viewport-spanning oval. This is a pure hardening fix. | Positive |
| A05 Security Misconfiguration | N/A | None |

**Verdict: SHIP** — clean. No security issues. The defensive input validation (`clampUnit`, `Number.isFinite`, `Math.min` cap) is good practice.

---

### 2.5 `e1c075b` — cli ensure v3 collection

**Files**: `src-tauri/src/cli.rs`

| OWASP Category | Finding | Severity |
|----------------|---------|----------|
| A03 Injection | `ensure_collection_v3` calls a pre-defined Qdrant collection name constant (`COLLECTION_V3`). No user input enters the Qdrant HTTP request URL beyond the collection name constant. | None |
| A01 Auth | CLI subcommand only — not accessible via IPC from the frontend. | N/A |
| A05 Security Misconfiguration | `MEMEX_QDRANT_HTTP` env var controls the Qdrant endpoint URL. This is unchanged from pre-existing code and represents an acceptable local service configuration path. | Low (pre-existing) |
| A09 Logging | The report format string now uses `COLLECTION_V3` — no sensitive information in output. | None |
| A06 Vulnerable Components | No new dependencies. `crud::ensure_collection_v3` is a fork-only symbol. | None |

**Dependency note**: `crud` module is fork-only. Cherry-pick to upstream requires that module. Same backport constraint as 2b59dc9.

**Verdict: SHIP** — no security issues in the change itself.

---

### 2.6 `84db1fc` — enable WebView devtools

**Files**: `src-tauri/Cargo.toml`, `src-tauri/src/commands.rs`, `src/main.js`

This commit has a security-relevant change that requires detailed analysis. See §4.

The non-devtools changes (`tail_recent_errors` noise filter, errors-badge tooltip) are security-neutral or positive:
- The `tail_recent_errors` filter change tightens which errors surface to the banner. It does not widen attack surface.
- The `errors-badge` tooltip adds a `title=` attribute with a static string — no user-controlled data injected. Safe.

**Verdict on `tail_recent_errors` change**: Positive security posture improvement. Removing the broad regex match (`error:|traceback|panic` in body text) reduces false-positive recall that could mislead users about genuine errors.

**Verdict on devtools feature**: BLOCK (see §4).

---

## §3 Deep Dive: `f55d417` Threat Model Exercise

### 3.1 KF-01 Path Sandbox — Bypass Vectors

**Symlink attack (inside→outside)**

`sec.rs:contains()` calls `p.canonicalize()` which resolves all symlinks. The returned canonical path is then checked against `starts_with(root)`. The test `t_symlink_outside` (sec.rs:197-202) confirms this works correctly: a symlink inside the sandbox pointing to an outside file resolves to the outside canonical path and is rejected.

**Finding**: Symlink escape is correctly blocked. No bypass via symlink.

**Path traversal via `..`**

`canonicalize()` resolves `..` components. A path `~/.claude/projects/abc/../../etc/passwd` canonicalizes to `/etc/passwd` which fails `starts_with(root)`. Confirmed by `t_path_traversal_dotdot` test.

**Finding**: Path traversal is correctly blocked.

**Race condition (TOCTOU on session reads)**

For `get_session_turns` and `predict_next_actions`, the flow is:
1. `validate_session_path(p)` calls `canonicalize(p)` — REQUIRES the file to exist (canonicalize fails on non-existent files on Linux/macOS)
2. Returns canonical path
3. `parser::parse_session(&validated)` reads the file

Between steps 2 and 3, an attacker could move/delete the file. However:
- This is a read operation. Even if the file changes, the parser reads what's there. Worst case: parse error, not sandbox escape.
- The file must have been resolvable to the sandbox directory at step 1. Moving it afterward doesn't help the attacker read outside the sandbox.

**Finding**: No meaningful TOCTOU risk on read paths.

**Race condition (TOCTOU on snapshot export write)**

For `snapshot_export`, the flow is:
1. `sb.validate_path(&path, SnapshotOp::Export)` — checks `!canonical.exists()`
2. Returns `parent_canon.join(fname)` (canonical, parent dir resolved)
3. `indexer::snapshot_export(&canonical)` — calls `tokio::fs::write(dest, bytes)`

Between steps 1 and 3, an attacker with write access to the sandbox directory could create a symlink at exactly the canonical path pointing to an arbitrary file. `tokio::fs::write` follows symlinks and would overwrite the symlink target.

**Finding**: TOCTOU symlink race on export. Severity is LOW in practice because:
- The attacker must already have write access to `~/Library/Application Support/dev.sgwannabe.memex/snapshots/`
- If they have that access, they could directly manipulate snapshot files anyway
- The snapshot directory is user-owned with 0700 permissions by default on macOS

**Recommended mitigation (BLOCK for upstream if this is a concern)**:

```rust
// In indexer::snapshot_export, replace tokio::fs::write with O_EXCL atomic create:
use std::fs::OpenOptions;
let mut f = OpenOptions::new()
    .write(true)
    .create_new(true)  // O_CREAT | O_EXCL — fails if path exists (catches race)
    .open(dest)
    .with_context(|| format!("create_new {}", dest.display()))?;
f.write_all(&bytes)?;
```

### 3.2 KF-02 Snapshot Sandbox — Bypass Vectors

**`..` in filename (pre-canonicalize check)**

`validate_path` explicitly iterates `p.components()` and rejects `ParentDir` components before canonicalization. This catches `../foo.snapshot` even when the parent directory canonicalization would resolve it.

**Finding**: Pre-canonicalize traversal rejection is correct.

**Filename-only `..` bypass**

If a filename literally contains `..` as part of its name (e.g., `foo..snapshot`), this does NOT match `Component::ParentDir` (which only matches exact `..` path segments). It passes the component check but `fname.ends_with(".snapshot")` requires the `.snapshot` extension. `foo..snapshot` would pass — this is acceptable; it's an unusual but valid filename with no security impact since the parent directory is already validated inside the sandbox.

**Finding**: No bypass via embedded dots in filename.

### 3.3 KF-03 Signed Envelope — Signing Scheme Weakness

**The scheme uses plain SHA-256 of the blob, stored as JSON in a sidecar `.sig` file.**

This is an **unauthenticated integrity check**, not a cryptographic signature. The distinction matters:

- An attacker who can write to the snapshot directory can:
  1. Read `foo.snapshot`
  2. Construct a malicious replacement blob
  3. Compute SHA-256 of the replacement
  4. Overwrite `foo.snapshot` with the malicious blob
  5. Overwrite `foo.snapshot.sig` with a new JSON containing the correct SHA-256

After these steps, `SignedEnvelope::verify` returns `VerifyOutcome::Ok`. The "signature" is defeated.

**This is a local filesystem threat model.** On macOS, the snapshot directory is user-owned. The only scenario where this matters:
- A malicious app running as the same user could tamper with snapshots
- A compromised Qdrant backup tool with write access to the directory

For the stated goal of the system ("detect tampered or corrupted snapshots"), this scheme provides corruption detection but NOT tamper resistance against a motivated local attacker.

**Severity: Medium** — the scheme is architecturally weaker than claimed. The commit message describes it as "signed envelope" which implies tamper-resistance. It is not.

**Recommended mitigation for upstream PR**: Either:

Option A — Rename honestly (low effort):
```rust
// In snapshot.rs - rename struct and update documentation
pub struct IntegrityEnvelope; // Was: SignedEnvelope
// Document as: "SHA-256 integrity check — detects accidental corruption,
// not tamper-resistant against local write access"
```

Option B — HMAC with per-install key (strong, more effort):
```rust
use hmac::{Hmac, Mac};
use sha2::Sha256;

// On first run, derive a per-install key from system entropy and store in
// macOS Keychain (SecItemAdd/SecItemCopyMatching). The HMAC then requires
// both read AND knowledge of the Keychain-stored secret to forge.
type HmacSha256 = Hmac<Sha256>;
```

For upstream PR purposes: Option A (honest naming) is the minimum acceptable change. Document the limitation clearly so upstream maintainers make an informed decision.

### 3.4 Coverage Gap: Unvalidated Path Parameters

Two existing IPC commands accept arbitrary paths from the frontend without sandbox validation:

**`tail_recent_errors` (commands.rs:596)**:
```rust
pub async fn tail_recent_errors(
    path: Option<PathBuf>,  // NO VALIDATION
    since_seconds: Option<u64>,
```

**`list_sessions` (commands.rs:548)**:
```rust
pub async fn list_sessions(
    path: Option<PathBuf>,  // NO VALIDATION  
    limit: Option<usize>,
```

With `withGlobalTauri = true` and `csp: null`, any JavaScript executing in the WebView can call these with arbitrary paths. Since the frontend never passes the `path` parameter (it uses `None`/default), this is not currently exploited — but it is an open attack surface if XSS is ever achieved or if a malicious deep-link URI triggers frontend JavaScript.

The `.jsonl` extension filter in `tail_recent_errors` (line 614) limits data exfiltration, but an attacker could still enumerate filesystem structure.

**Required fix before upstream PR:**
```rust
// In tail_recent_errors and list_sessions, validate the optional path:
let root = if let Some(p) = path {
    SandboxRoot::from_env()
        .map_err(stringify)?
        .contains(&p)
        .map_err(|_| "path outside sandbox".to_string())?
} else {
    default_projects_root()
};
```

---

## §4 Deep Dive: `84db1fc` — WebView DevTools in Production

### The Risk

`src-tauri/Cargo.toml` adds `"devtools"` to the Tauri feature list:

```toml
- tauri = { version = "2", features = ["tray-icon"] }
+ tauri = { version = "2", features = ["tray-icon", "devtools"] }
```

In Tauri 2, the `devtools` feature compiles the WebView inspector into **all build configurations** — there is no automatic debug-only guard. The commit message acknowledges this: *"can be turned off again for the hackathon final binary if size matters."*

**What this enables for any user of the installed .app:**
- Right-click → Inspect Element
- Full DevTools access: Elements, Console, Network, Sources, Application tabs
- The Console tab provides a JavaScript REPL with access to `window.__TAURI__.invoke()`
- From the Console, any user can call any registered Tauri IPC command

**IPC commands accessible via DevTools Console:**

```javascript
// Any of these are now trivially callable by anyone with physical access:
window.__TAURI__.invoke("snapshot_import", { path: "/any/path.snapshot" })
window.__TAURI__.invoke("list_sessions", { path: "/etc" })
window.__TAURI__.invoke("tail_recent_errors", { path: "/Users/victim" })
```

The path sandbox (f55d417) mitigates some of this, but:
- `list_sessions` and `tail_recent_errors` have no sandbox validation (see §3.4)
- DevTools Console bypasses all Tauri capability restrictions because it executes in the same JS context as the app

**For `sgwannabe/memex` upstream**: This is a desktop developer tool used by a single authenticated user. The threat model is different from a web server. Physical console access is generally trusted. However, the security posture is still worse than necessary.

**Upstream maintainer impact**: If upstream accepts this PR and ships a `.dmg` with devtools enabled, any `.app` installed on a shared Mac (lab, conference, paired coding) exposes all of the above.

### Concrete Recommendation

Strip the `devtools` feature from any upstream PR. The debugging purpose that motivated `84db1fc` was for in-development diagnosis of the purple oval bug — that bug is now fixed by `deed283`. DevTools are no longer needed.

If upstream maintainers want to preserve an optional debugging path, the correct pattern is:

**Option 1 — Conditional compilation (recommended)**:

`src-tauri/Cargo.toml`:
```toml
[features]
default = ["tray-icon"]
debug-inspector = ["tauri/devtools"]
tray-icon = ["tauri/tray-icon"]
```

`src-tauri/src/lib.rs`:
```rust
#[cfg(feature = "debug-inspector")]
{
    // Only allow devtools on debug builds when explicitly opted in
    if cfg!(debug_assertions) {
        app.get_webview_window("main")
            .map(|w| w.open_devtools());
    }
}
```

Build-time activation:
```bash
cargo tauri build --features debug-inspector  # developer only; never in CI release
```

**Option 2 — Runtime environment variable gate**:

`src-tauri/Cargo.toml`: Remove `devtools` from features (production build).

If devtools is needed for a specific diagnosis session, rebuild with the feature. This is what the commit message acknowledges but does not implement.

**Option 3 — Tauri 2 native approach** (preferred if upstream is on macOS only):

Tauri 2 exposes `Window::open_devtools()` / `Window::close_devtools()` as methods gated by `#[cfg(debug_assertions)]` automatically when the `devtools` feature is enabled. If the app is built in release mode (`--release`), `debug_assertions` is false, so `open_devtools()` is a no-op even with the feature compiled in. However, the **inspector UI is still bundled** and accessible via right-click in WKWebView on macOS regardless of `debug_assertions`. This is a Tauri 2 / WebKit limitation — the right-click inspector cannot be fully disabled at the feature level on macOS WKWebView.

**Definitive recommendation for upstream PR**: The `devtools` feature addition in `84db1fc` must be **removed from any upstream PR**. Other changes in the commit (tail_recent_errors filter, errors-badge tooltip) can ship.

---

## §5 Compliance Summary (OWASP Top 10 Pass/Fail per Commit)

| Check | e402b1f | f55d417 | 2b59dc9 | deed283 | e1c075b | 84db1fc |
|-------|---------|---------|---------|---------|---------|---------|
| A01 Broken Access Control | Pass | Pass+ | Pass | Pass | Pass | Fail (DevTools bypass) |
| A02 Cryptographic Failures | N/A | Partial (SHA-256, not HMAC) | N/A | N/A | N/A | N/A |
| A03 Injection | Pass | Pass+ | Pass | Pass | Pass | Pass |
| A04 Insecure Design | Minor (innerHTML) | Pass+ (sandbox) | Pass | Pass | Pass | Fail (DevTools) |
| A05 Security Misconfiguration | Pass | Low (hardcoded path) | Pass | Pass | Pass | Fail (csp:null + devtools) |
| A06 Vulnerable Components | Pass | Pass | Pass | Pass | Pass | Pass |
| A07 Auth Failures | N/A | N/A | N/A | N/A | N/A | N/A |
| A08 Software/Data Integrity | N/A | Partial (unauthenticated hash) | N/A | N/A | N/A | N/A |
| A09 Insufficient Logging | Pass | Pass | Pass | Pass | Pass | Pass+ |
| A10 SSRF | Pass | Pass | Pass | Pass | Pass | Pass |

---

## §6 Final Verdicts

### `deed283` — SHIP
No security issues. Purely defensive. Fixes a score-clamping bug that could have produced DOM pollution via anomalous SVG attributes. The `clampUnit`/`Number.isFinite`/`Math.min` layered defense is appropriate. Safe to include in any upstream PR.

### `e1c075b` — SHIP
No security issues. Correct collection lifecycle management. Depends on fork-only `crud` and `schema` modules — document as a backport dependency.

### `e402b1f` — SHIP_WITH_CHANGES
XSS is handled via `escapeHtml`. The recommended change is to replace the `meta.innerHTML` template literal with DOM `createElement`/`textContent` construction to make XSS structurally impossible rather than policy-dependent. Apply this in the backport rewrite (since this commit requires a manual rewrite anyway per UPSTREAM_PR_PLAN.md §2 Candidate B).

### `2b59dc9` — SHIP_WITH_CHANGES
No security issues in the change itself. The `source_agent` string comparison is safe. Required change: document the `codex_parser` dependency for upstream reviewers. Cannot ship to upstream without first shipping the codex parser module.

### `f55d417` — SHIP_WITH_CHANGES
The core sandbox controls (KF-01, KF-02) are correct and well-tested. Three required changes before upstream PR:
1. **High**: Gate `tail_recent_errors` and `list_sessions` optional `path` parameter through `SandboxRoot::contains()` (§3.4)
2. **Medium**: Rename `SignedEnvelope` → `IntegrityEnvelope` or add prominent documentation that SHA-256 sidecar is not tamper-resistant (§3.3). Alternatively implement HMAC with Keychain-backed key.
3. **Low**: Implement `O_EXCL` create in `snapshot_export` to eliminate the TOCTOU window (§3.1).

### `84db1fc` — BLOCK
The `devtools` feature addition must not go to upstream. It enables the WebView inspector in all build configurations, providing anyone with access to the installed app a JavaScript console with full `window.__TAURI__.invoke()` access, bypassing Tauri's capability layer. The other two changes in this commit (tail_recent_errors noise filter, errors-badge tooltip) are safe and can be extracted into a separate PR.

**Blocked change**: `src-tauri/Cargo.toml` line adding `"devtools"` to `tauri` features.
**Extractable changes**: `src-tauri/src/commands.rs` tail_recent_errors filter + `src/main.js` errors-badge tooltip. These can be submitted as a standalone upstream PR with no security concerns.

---

## §7 Issues Deferred from f55d417 (Tracking)

The original commit deferred these items — confirming they remain open:

| ID | Description | Status |
|----|-------------|--------|
| MED-2 | Streaming SHA-256 for large snapshots | Open — current `std::fs::read` loads entire blob into RAM |
| LOW-1 | Canonicalize final import path to catch sandbox-internal symlinks | Open — validate_path for Import returns `parent_canon.join(fname)`, fname is the literal filename component. If fname is itself a symlink, it isn't resolved. |
| LOW-2 | Surface all-neighbour-rejected in PredictionContext.warn | Open |
| NIT-1 | Derive snapshot path from `app_data_dir()` | Open |
| NIT-2 | Trim canonical path from sandbox error message | Open |

**LOW-1 clarification**: `validate_path` for Import calls `canonical.exists()` on `parent_canon.join(fname)`. If `fname` is a symlink file within the sandbox pointing outside the sandbox, it exists, passes the Import check, and the caller receives the unresolved path. `indexer::snapshot_import` then reads it — following the symlink. This is the partner TOCTOU/symlink issue to the Export one identified in §3.1. Same mitigation applies: use `O_NOFOLLOW` or re-canonicalize the final path on import.

---

*Security review by claude-sonnet-4-6 on 2026-05-19. All file:line references are against the fork HEAD at commit `8509096`.*
