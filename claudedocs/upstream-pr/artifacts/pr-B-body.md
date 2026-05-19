# PR-B — KF-01: path sandbox for IPC session-file reads

**Target:** `sgwannabe/memex` `main` (@ `4973a91`)
**Head:** `ComBba/memex` `upstream-pr/kf01-path-sandbox` (@ `fc7d6cd`)

---

## Title

```
feat(security): sandbox session-file reads under ~/.claude/projects
```

(63 chars)

## Body

```markdown
## Summary

Add a small `SandboxRoot` validator that constrains every session-JSONL
read invoked through Tauri IPC to a single allow-listed directory:
`~/.claude/projects`. All other paths — including symlinks pointing
outside the root, `..` traversal, and embedded NUL bytes — are rejected
before `parser::parse_session` is invoked.

Closes an arbitrary-file-read class of bug where a tampered Qdrant
payload, malicious deep-link, or compromised renderer can pass an
attacker-controlled `source_path` to one of four IPC commands and cause
the Memex backend to open and parse arbitrary user-readable files
(`/etc/passwd`, `~/.ssh/id_rsa`, browser cookies, etc.).

## Problem / Motivation

Four IPC commands on `upstream/main` resolve a filesystem path from
either a Qdrant payload field or an optional IPC parameter, and feed
that path straight to `parser::parse_session` / `WalkDir`:

| File:line                            | Function                  | Path source                |
|---|---|---|
| `src-tauri/src/commands.rs:195`      | `get_session_turns`       | Qdrant payload `source_path` |
| `src-tauri/src/indexer.rs:1267`      | `predict_next_actions` (active session)   | Qdrant payload `source_path` |
| `src-tauri/src/indexer.rs:1344`      | `predict_next_actions` (neighbor walk)    | Qdrant payload `source_path` |
| `src-tauri/src/commands.rs:551`      | `tail_recent_errors`      | optional IPC `path` parameter |
| `src-tauri/src/commands.rs:481`      | `list_sessions`           | optional IPC `path` parameter |

None of these validate the path before reading it. With
`withGlobalTauri = true` and `csp: null` in `tauri.conf.json`, any
JavaScript executing in the WebView (legitimate frontend, a future XSS,
a malicious deep-link handler) can call:

```javascript
window.__TAURI__.invoke("tail_recent_errors", { path: "/etc" })
window.__TAURI__.invoke("list_sessions", { path: "/Users/victim/.ssh" })
```

…and the Memex backend will obligingly walk those trees.

The Qdrant-payload paths are a separate but related vector: if an
attacker can write a single point into the user's local Qdrant (via a
malicious snapshot import, a network-exposed Qdrant without auth, or a
compromised MCP server upserting points), they can choose any
`source_path` string and Memex will `open(2)` it the next time
`get_session_turns` or `predict_next_actions` references that session.

### Reproduction (illustrative)

```ts
// Inside the running app's DevTools console (or any extension-injected
// script if XSS is ever achieved):
await window.__TAURI__.invoke("get_session_turns", {
  session_id: "<id-of-a-point-whose-payload-you-tampered>"
});
// → backend reads /etc/passwd; parse_session yields nothing useful but
//   the file IS opened and read into memory.
```

After this PR the call is rejected at the IPC boundary with a clear
`"path outside sandbox"` error string and the file is never opened.

## Solution

New module `src-tauri/src/sec.rs` (~370 LOC including tests). Public
surface:

- `SandboxRoot::from_env()` — discovers the sandbox root via
  `dirs::home_dir().join(".claude").join("projects")` and canonicalizes
  it once. Returns `Err` if the home directory cannot be resolved or if
  the projects directory doesn't exist (a nonexistent root is treated
  as a hard error so the user gets a clear message instead of every
  path being "outside the sandbox").
- `SandboxRoot::contains(p: &Path) -> Result<PathBuf>` — the workhorse.
  Rejects:
  - empty paths
  - paths containing NUL bytes (Unix; on Windows `Path::new` rejects
    them at the type level)
  - paths where `canonicalize()` fails (typically: nonexistent file)
  - canonical paths that don't start with the sandbox root — this is
    the check that defeats symlink-escape and post-`..` traversal.
- `validate_session_path(p: &Path) -> Result<PathBuf>` — convenience
  wrapper used by every IPC entry point. Returns the canonical path so
  callers feed the locked-in resolved target to `parse_session`
  (closes a TOCTOU between validation and read).

Call-site wiring (additive — no signature changes):

| File:line | Change |
|---|---|
| `commands.rs:195` `get_session_turns` | `validate_session_path(&source)` → `parse_session(&validated)` |
| `commands.rs:481` `list_sessions` (optional `path` param) | `validate_session_path(&p)` before passing to `scan_dir` |
| `commands.rs:551` `tail_recent_errors` (optional `path` param) | `validate_session_path(&p)` before `WalkDir::new(&root)` |
| `indexer.rs:1267` `predict_next_actions` (active session) | `validate_session_path(&source_path)` → `parse_session(&validated)` |
| `indexer.rs:1344` `predict_next_actions` (neighbor loop) | `validate_session_path` per neighbor; on rejection, skip silently and continue (a tampered payload from one neighbor shouldn't kill the whole prediction) |

`lib.rs` adds `pub mod sec;`.

## Test coverage

20 unit tests in `src-tauri/src/sec.rs` (`#[cfg(test)] mod tests`):

- `t_valid_claude_session_path` — happy path under tempdir sandbox
- `t_path_outside_sandbox_etc` — `/etc/passwd` rejected, error contains `"outside sandbox"`
- `t_path_outside_both_tmp` — file in a different tempdir is rejected
- `t_path_traversal_dotdot` / `t_path_traversal_double_dotdot` — `..` segments resolved by canonicalize, then rejected
- `t_symlink_outside` — symlink inside root → external target: rejected (KEY SECURITY ASSERTION)
- `t_symlink_inside` — symlink inside root → in-sandbox target: accepted
- `t_symlink_dangling` — symlink to nonexistent target: rejected by canonicalize
- `t_nul_byte_path` — NUL byte rejected pre-canonicalize, error contains `"NUL"`
- `t_empty_string` — empty path rejected, error contains `"empty"`
- `t_nonexistent_path` — nonexistent path inside sandbox rejected
- `t_canonical_idempotent` — `contains(contains(p))` returns the same canonical path
- `t_unicode_path_valid` — `세션-한국어-😀.jsonl` accepted
- `t_long_path_no_panic` — 4096-char path doesn't panic
- `t_arbitrary_bytes_no_panic` — invalid UTF-8 byte sequences don't panic
- `t_path_with_spaces` — paths with spaces accepted
- `t_rejects_directory_outside_sandbox` / `t_accepts_directory_inside_sandbox` — `contains` is file/dir-agnostic
- `t_root_accessor_returns_canonical` — `root()` returns the canonicalized form
- `t_error_message_does_not_leak_etc_contents` — error strings don't accidentally embed file content

8 portable integration tests in `src-tauri/tests/sec_integration.rs`
exercising the public `validate_session_path` via a tempdir-backed
sandbox + `SandboxRoot::from_root`. CI-safe: none of these touch the
real `$HOME`.

2 additional `#[ignore]`-gated integration tests for verification on a
developer machine where `~/.claude/projects` actually exists. Run with
`cargo test -- --ignored`.

Local results:

```
$ cargo check
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.24s

$ cargo test --lib sec::
   test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured

$ cargo test --test sec_integration
   test result: ok. 8 passed; 0 failed; 2 ignored; 0 measured
```

## Trade-offs / Alternatives considered

- **Use the OS file-dialog as the only entry point.** Doesn't cover
  Qdrant-payload paths or programmatic IPC.
- **Plain string prefix check (no canonicalize).** Trivially bypassed
  with symlinks or `..`.
- **Pull in `cap-std`.** Considered, but `cap-std` is a heavier
  dependency than the ~150 LOC of `std`-only Rust needed here, and the
  validation surface is small enough to audit by eye.
- **Make the root configurable** (e.g., `sandbox.toml`). Deferred —
  upstream today has a single hard-coded root, so a fixed-root sandbox
  matches current expectations.
- **Newtype `SandboxPath(PathBuf)` enforced by the type system.** A
  cleaner long-term design, but a larger refactor. Current approach
  requires reviewer discipline: every new file-reading command must call
  `validate_session_path`. The four affected call sites now carry a
  `// SECURITY (KF-01):` comment to flag the pattern.

## Backwards compatibility

- **Default-config users** (`~/.claude/projects` exists, no IPC-supplied
  `path` parameter): zero behavior change. `validate_session_path`
  succeeds on every legitimate path under the root, returns the
  canonical form, and `parse_session` gets exactly what it would have
  gotten before.
- **Users without `~/.claude/projects`** (fresh install, no Claude
  sessions yet): `SandboxRoot::from_env()` returns `Err("canonicalize
  sandbox root <path>")`. Upstream today would call
  `default_projects_root()` and return an empty result; the new
  behavior surfaces a clearer error. **This is a string change visible
  to the frontend on a corner case** — no schema or command-signature
  change.
- **Symlinked session directories** (rare but exists): a user who
  symlinks `~/.claude/projects` to `/Volumes/External/claude-projects`
  is fine — `canonicalize` resolves the link once at sandbox
  construction, and subsequent containment checks canonicalize the
  candidate path against the canonical root.
- **No Tauri command return-type changes.** All four wired commands
  keep their existing signatures.

## Cargo.toml impact

| Change | Why |
|---|---|
| `+ dirs = "5"` (runtime) | Cross-platform home-directory resolution (`$HOME` on Unix, `%USERPROFILE%` on Windows). Used by `SandboxRoot::from_env`. `dirs` is widely adopted (rustup, cargo). |
| `+ tempfile = "3"` (dev-dependencies) | Test-only; not shipped in release binary. Used to back the unit + integration test sandboxes. |

No `sha2` — that crate is for snapshot signing (separate proposal) and
is not pulled in by this PR.

## What is NOT changed

Building trust by being explicit about the diff boundary:

- `parser.rs`, `mcp.rs`, `watcher.rs`, `cli.rs` — untouched.
- Tauri command signatures — every one of the four wired commands
  keeps its existing parameter list and return type.
- Frontend (`src/index.html`, `src/main.js`, `src/styles.css`) —
  untouched.
- `snapshot_export` / `snapshot_import` — untouched (those are
  separate audit items in our internal review; future PR.).
- Qdrant schema and collection layout — untouched.

## Screenshots / recordings

Backend-only change; no UI screenshots. Attached to the PR:

- [ ] `cargo test --lib sec::` output (20 passed)
- [ ] `cargo test --test sec_integration` output (8 passed + 2 ignored)
- [ ] DevTools recording: `invoke("get_session_turns", { session_id:
      "<tampered-point>" })` before vs after — before: opens
      `/etc/passwd`; after: returns `"path outside sandbox"` error
      string.

## Manual verification

```
1. cargo build --release
2. Open the app, click any session in the Time Machine stack — works
   as before, no behavior change.
3. In DevTools console:
      await window.__TAURI__.invoke("list_sessions", { path: "/etc" })
   EXPECT: rejection with "path outside sandbox" (was: empty result
   silently, with a directory walk over /etc).
4. With Qdrant CLI, manually upsert a point whose payload.source_path =
   "/etc/passwd", note its session_id, then:
      await window.__TAURI__.invoke("get_session_turns", { session_id: "<that-id>" })
   EXPECT: "path outside sandbox" error (was: file read, parse error).
5. Symlink test: ln -s /etc/passwd ~/.claude/projects/test/evil.jsonl,
   try to load it via any command. EXPECT: rejection.
6. Fresh-user simulation: rm -rf ~/.claude/projects (in a sandbox VM!),
   restart Memex. EXPECT: list_sessions returns a clear "canonicalize
   sandbox root" error instead of a silent empty list. rm the symlink.
```

## Future work (out of scope for this PR)

- Snapshot path sandbox + signed envelope (separate audit items;
  would be PR-C in the same series).
- Newtype-enforced `SandboxPath` so the compiler catches a new IPC
  command that forgets to validate.
- Windows portability beyond `dirs::home_dir` (UNC paths, junction
  points, drive letters) — `dirs` handles the home directory but
  `canonicalize` on Windows has its own quirks (UNC prefix `\\?\`) that
  this PR does not exercise. Recommend a Windows CI row before claiming
  Windows support.

## References

- Inspired by an internal security audit of `ComBba/memex@f55d417` +
  hardening from `ComBba/memex@769af65`. This PR is a clean re-write
  against `upstream/main` with the fork-only `SourceAgent` enum and
  Codex-second-root dropped (upstream doesn't index Codex; a future
  proposal can extend the sandbox if Codex parsing lands).
- The vulnerable line on `upstream/main`:
  https://github.com/sgwannabe/memex/blob/4973a91/src-tauri/src/commands.rs#L195
```

## Commit message

```
feat(security): path sandbox for IPC file inputs (KF-01)

Restrict every filesystem path that arrives via Tauri IPC to a
sandbox root under `~/.claude/projects` (resolved via `dirs::home_dir()`
for cross-platform portability). After canonicalization, re-check that
the path still lives under the sandbox to defeat symlink escape.

Routes through `sec::validate_session_path()`:
- get_session_turns
- predict_next_actions (active + neighbor loop)
- tail_recent_errors
- list_sessions

Adds `dirs = "5"` (runtime) and `tempfile = "3"` (dev-only).
20 unit tests + 8 portable integration tests + 2 #[ignore]-gated
integration tests that touch real $HOME (run with --ignored).
```

## Suggested labels

`security`, `enhancement`, `backend`, `rust`

## Reviewer suggestions

- **Security-minded reviewer** — interested in path-handling
  correctness, symlink behavior, and Tauri IPC trust boundaries.
- **Rust reviewer** familiar with `std::path::Path::canonicalize`
  quirks on macOS (e.g., `/var` → `/private/var`) and with the
  existing `commands.rs` IPC surface.
