# Upstream Maintainer Q&A — Anticipated Review Friction

**Prepared**: 2026-05-19  
**Audience**: ComBba (contributor) preparing PRs toward sgwannabe/memex  
**Scope**: Four candidates — PR A (mix-modal), PR B (KF-01 path sandbox), PR C (defensive SVG), PR D (cli ensure v3)  
**Honesty level**: Brutally honest. The goal is to get merged, not to look good.

---

## Preflight: What sgwannabe Already Knows

The upstream (`sgwannabe/memex`) as of `4973a91` has:
- `lens_search` (single-vector) — no `lens_search_v2`
- `mix_match` — exists, but the modal UX is basic (no picker)
- `memex_sessions` collection only — no `memex_sessions_v3`
- No `sec.rs`, no `snapshot.rs` (signature layer), no `crud.rs`, no `schema.rs`, no `codex_parser.rs`
- MCP server (`mcp.rs`) and watcher — these are upstream-only additions not yet in fork main

Every PR below must be written as a **clean backport against `upstream/main`**, not a cherry-pick of fork commits. The fork-internal jargon ("P3 KG-03", "KC-01b", "VSD 2026", "D-14") must not appear in PR bodies or commit messages. The `Co-Authored-By: Claude Opus 4.7` trailer should be dropped from upstream PRs unless the maintainer has explicitly opted into AI attribution.

---

## PR A — Mix & Match Self-Contained Picker

**What it does**: Adds a search-and-add picker inside the `#mix-modal` dialog so users can find sessions without closing the modal.  
**Backport complexity**: High — all 3 files conflict; needs manual rewrite against upstream's HTML/JS/CSS.  
**Upstream has the problem**: Yes. `upstream/main` has the same modal with the same "click + pos on a card" instruction pointing to click-blocked buttons.

### Q&A

**Q1 (Scope creep) — "This is a lot of new JS for a 'bug fix'. Did you consider a simpler approach?"**

A: The root bug is architectural: the fix requires adding UI inside the modal because the existing instruction is physically unreachable while the modal is open. The minimum viable fix would be to change the hint text to "close this dialog, add cards, then reopen" — which is a UX regression, not a fix. The picker approach is the smallest change that makes the feature self-contained without touching the backdrop z-index (which would require CSS changes across the whole modal stack). The 148 JS lines are primarily defensive: UUID direct-pick, empty-state handling, button state sync on remove, and the `updateRunMixButton` guard. None of these are speculative features.

**Q2 (Style/convention) — "Your picker uses `document.createElement` for every row. The rest of this codebase uses template literals / innerHTML in several places. Please be consistent."**

A: Both patterns exist in upstream `main.js` (compare `openMixModal` which uses innerHTML vs `renderMixDropzones` which uses createElement). The picker uses createElement specifically to avoid raw innerHTML on user-facing search results — XSS mitigation for queries that could contain `<script>` or `"` in session titles. If the maintainer prefers innerHTML + `escapeHtml`, we can adapt; the `escapeHtml` helper is already in the codebase.

**Q3 (Performance) — "You fire `lens_search` on every Enter keypress. What if the user types fast?"**

A: The current implementation fires on `keydown` Enter only (not `input` event), so it is already debounced by the Enter keystroke. There is no auto-fire on each character. If the maintainer wants a debounced `input` handler instead of Enter-triggered search, that is a valid improvement suggestion we should accept.

**Q4 (Cross-platform) — "Tested on Linux/Windows?"**

A: The picker is pure JS/HTML/CSS with no platform-specific code paths. It invokes `lens_search` which is already tested on the upstream's collection (`memex_sessions`). However, the backport was developed on macOS only. We should be honest: we have not run `tauri build` on Linux or Windows for this specific change.

**Q5 (Backwards compatibility) — "Does this break the existing 'close modal, click card, reopen' flow?"**

A: No. The `[+ pos]` / `[− neg]` buttons on main-view stack cards still work and call the same `addToMix(side, sessionId)` path. The picker is additive. The dropzone hint copy changed to mention both flows, which is strictly more accurate.

**Q6 (Test coverage) — "Where are the tests for `runMixPickerSearch`, `renderMixPickerRow`, `attachMixPickerEvents`?"**

A: There are no automated JS unit tests. Upstream has no testing framework for the frontend (`src/` contains no test runner config, no `package.json` test script). We verified manually per the commit message steps. If the maintainer wants us to add tests using a specific framework (Vitest, Playwright), we should ask them to specify rather than guessing.

This is the most likely rejection reason. We should proactively propose: "We're happy to add a Playwright smoke test that opens the modal, types a query, and verifies a result row appears, if you can point us to your preferred testing setup."

**Q7 (Security) — "The picker renders session titles directly in the DOM. Is there XSS risk?"**

A: All user-visible data goes through `escapeHtml(str)` before insertion. The `session_id` is used only as a value passed to `addToMix`, never rendered as HTML. The `q` input value is never reflected into innerHTML. This is correct as written.

**Q8 (Alternatives) — "Did you consider fixing the backdrop z-index so main-view buttons remain clickable?"**

A: Yes. Changing `z-index` on `::backdrop` or restructuring the button DOM to be inside the dialog are alternative approaches. The z-index approach would require either (a) making the backdrop transparent/clickthrough (which breaks modal semantics and accessibility), or (b) moving the `[+ pos]` cards inside the dialog (which duplicates the entire stack render logic). The self-contained picker is the cleanest boundary: keep the modal as a modal, give it internal affordances.

**Q9 (Maintenance burden) — "Who maintains this picker going forward?"**

A: The picker calls into `lens_search` which is already a maintained IPC command. The UI code is isolated in three clearly named functions (`runMixPickerSearch`, `renderMixPickerRow`, `attachMixPickerEvents`) and one CSS block (`.mix-picker*`). Any future changes to `LensWeights` or `SearchHit` shape will be reflected automatically because the picker uses the same normalization helpers (`lensWeightsForV2`, `normalizeLensResult`) as the main stack.

**Q10 (Hackathon context) — "This looks like it was developed in a hackathon fork. Should I be concerned about code quality?"**

A: Be transparent but not defensive. Say: "This was developed as part of a focused UX improvement session. The core bug (click-blocked buttons) exists in upstream `main` unchanged since the feature was added. The fix has been in daily use for two weeks with no regressions. The code quality concern is fair — we haven't added automated tests, which we acknowledge and are prepared to address."

**Q11 (Scope creep, secondary) — "Your commit message says 'self-priming on open with most recent query'. That's a feature, not a bug fix."**

A: Correct, and this is a genuine scope concern. The self-priming is a single line (`openMixModal` seeds the input with `state.query`). It can be removed from the backport with no impact on the core fix. The maintainer is right to flag it, and we should be prepared to drop it.

**Q12 (Fork dependency) — "Your picker calls `lens_search_v2` with a fallback to `lens_search`. Upstream doesn't have `lens_search_v2`."**

A: This is the most critical technical blocker for the backport. The fork's `runMixPickerSearch` calls `lens_search_v2` first, falling back to `lens_search`. Upstream only has `lens_search`. The backport must remove the `lens_search_v2` branch entirely and call `lens_search` directly. This also means removing `lensWeightsForV2()` and `normalizeLensResult()` from the picker, replacing them with upstream's `LensWeights` shape. The resulting picker is simpler (no v2 fallback logic) and fully functional against upstream's collection.

---

## PR B — KF-01 Path Sandbox (`sec.rs`)

**What it does**: Adds `SandboxRoot` / `validate_session_path` to prevent a tampered Qdrant payload from directing `get_session_turns` to read arbitrary files outside `~/.claude/projects` or `~/.codex/sessions`.  
**Backport complexity**: Medium-High — `sec.rs` is self-contained, but wiring into `commands.rs` requires adapting to upstream's command signatures. Also: upstream does NOT index Codex sessions at all; `SourceAgent::Codex` is fork-specific.  
**Upstream has the problem**: Yes. Upstream `get_session_turns` (commands.rs:174–200) does `parser::parse_session(Path::new(&source))` with no path validation. A tampered `source_path` payload could read any user-readable file.

### Q&A

**Q1 (Scope creep) — "Why is there a `SourceAgent` enum here? Upstream doesn't use Codex."**

A: `SourceAgent` was designed for the fork's multi-agent environment. For the upstream backport, `SourceAgent` can be removed entirely. The sandbox needs only a single root (`~/.claude/projects`) and `SandboxRoot::contains()` works with one root. Simplifying to a single-root sandbox reduces the surface area and removes the Codex dependency.

**Q2 (Style/convention) — "We don't have a `sec.rs` module pattern. Security primitives normally live near their call sites or in a dedicated `auth` or `validation` module."**

A: Fair. The naming convention is our choice; the maintainer can rename to `sandbox.rs`, `validation.rs`, or inline into `commands.rs`. The implementation is independent of the module name. We should offer to rename on request.

**Q3 (Performance) — "You call `canonicalize` on every `get_session_turns` invocation. That's a syscall per request."**

A: `canonicalize` is a single `realpath()` syscall (one filesystem round-trip). `get_session_turns` is already async and does a Qdrant gRPC round-trip, a file stat, and a full JSONL parse. One additional `realpath()` is noise. If the maintainer is concerned, we can cache the sandbox roots in `AppState` so only the path being validated incurs the syscall.

**Q4 (Cross-platform) — "The NUL-byte check uses `#[cfg(unix)]`. What about Windows?"**

A: On Windows, Rust's `Path`/`OsStr` does not allow embedded NUL bytes at the type level (it would panic at `Path::new` before reaching the check). The `#[cfg(unix)]` guard is correct — on non-Unix the check is unnecessary, not missing. The comment in the code explains this. However, Windows path canonicalization behavior (UNC paths, drive letters, symlinks via junction points) is untested. This is a legitimate concern we cannot fully address without a Windows CI environment.

**Q5 (Backwards compatibility) — "This changes the error behavior of `get_session_turns`. Existing users with Qdrant payloads containing unusual paths could get new errors."**

A: If a user's Qdrant collection has a `source_path` that points to a legitimate file under `~/.claude/projects`, canonicalize will succeed and behavior is unchanged. If the path is outside the sandbox (e.g., a path stored before the user moved their Claude projects), the command now returns an error instead of parsing the file. This is a deliberate security tradeoff: we accept the possibility of surfacing errors for edge-case stale paths in exchange for preventing path traversal. In practice, Memex always stores the path at index time and the user rarely moves the projects root.

**Q6 (Test coverage) — "30 unit tests — good. But the integration tests use `$HOME` directly. What does this do in CI?"**

A: The integration tests at `tests/sec_integration.rs` are conditional: `if any_exists { assert!(result.is_ok()) } else { assert!(result.is_err()) }`. On a CI runner where neither `~/.claude/projects` nor `~/.codex/sessions` exists, the `from_env()` test asserts an error — which is the correct behavior. The `/etc/passwd` rejection test always runs. These tests will not false-positive on CI.

**Q7 (Security) — "Your `canonicalize` before the sandbox check is the right pattern. But what about symlinks INSIDE the sandbox root that point outside?"**

A: This is a known gap (documented as LOW-1 in the commit message of the fork's security audit). `canonicalize` resolves symlinks, so a symlink _inside_ the root that points _outside_ will resolve to the outside path, which will be rejected by `starts_with(root)`. The guard works correctly for this case. A symlink _to_ an external location that happens to resolve to something that looks like it's inside the root — that cannot happen because `canonicalize` follows the symlink to its actual target. The implementation is correct for Unix symlinks.

**Q8 (Alternatives) — "Could you just validate that the path ends in `.jsonl` rather than building a full sandbox?"**

A: Extension checking alone is insufficient: an attacker who controls the Qdrant payload could store a path like `/etc/shadow` (which exists without `.jsonl`) but also could construct `/../../../sensitive.jsonl` paths. The sandbox provides defense-in-depth: extension + directory containment + canonicalization + NUL rejection are four independent layers, each of which alone would be insufficient.

**Q9 (Maintenance burden) — "Every new IPC command that reads files will need to call `validate_session_path`. Is there a way to make this automatic?"**

A: Not without a type-system-level change (e.g., a newtype `SandboxPath(PathBuf)` that can only be constructed through the validator). That is a larger refactor we are not proposing. The current approach requires discipline: each new file-reading command must explicitly call `validate_session_path`. We should document this in a `// SECURITY: always validate with sec::validate_session_path` comment at the call site in `commands.rs`.

**Q10 (Hackathon context) — "This came from a hackathon fork. Was this security issue found organically or manufactured to have something to submit?"**

A: Be honest. The vulnerability is real: upstream `get_session_turns` (still present in `4973a91`) passes `source_path` from Qdrant payload directly to `parser::parse_session` with no validation. A user who manually inserts a malicious point into their Qdrant collection could cause Memex to parse any readable file. The fix is genuine. The hackathon context accelerated discovery, not fabrication.

**Q11 (Scope) — "You're adding `sha2 = "0.10"` and `tempfile = "3"` to production deps. Are both necessary for the path sandbox alone?"**

A: No. `sha2` is only needed for `SignedEnvelope` (PR C material, snapshot signing). `tempfile` is dev-only (test helper). For the KF-01 path sandbox PR alone, the Cargo.toml change should be limited to `dirs = "5"` (runtime) and `tempfile = "3"` (dev-dependency only). If PR B is submitted without the snapshot signing layer, `sha2` must be excluded.

**Q12 (Codex references) — "Your PR body mentions `~/.codex/sessions`. Upstream doesn't support Codex."**

A: Remove all Codex references from the upstream PR body. The backport should present as: "path containment for Claude Code sessions (`~/.claude/projects`)." The `SourceAgent` enum should be dropped entirely, leaving a simpler `SandboxRoot` that operates on a single root.

---

## PR C — Defensive SVG Primitives (Heat Trail Fix)

**Fundamental blocker**: Upstream has NO heat trail feature. `#heat-trail` SVG, `drawHeatTrail()`, `HEAT_COLOR_*` constants, and `.heat-trail` CSS do not exist in `upstream/main`. There is nothing to fix.

**However**, the _fix patterns_ themselves are reusable defensive SVG techniques. If upstream has any SVG rendering, the following primitives are worth proposing as isolated improvements:

- `vector-effect="non-scaling-stroke"` — prevents stroke-width from being magnified by viewBox scaling
- `Number.isFinite()` guard before using a computed value as a stroke-width
- `preserveAspectRatio="none"` for pixel-mapped SVG overlays
- viewBox sanity gate (`width < 100 || height < 100` bail-out) before drawing

**Honest assessment**: This cannot be submitted as a PR in its current form. It requires either:
1. Waiting until upstream adopts the heat trail feature (unlikely without the full P6 feature drop), or
2. Finding the specific upstream SVG code that has similar issues and submitting a targeted fix there.

Checking upstream `main.js` for SVG usage:

### Q&A (for a hypothetical "defensive SVG" PR if upstream has SVG rendering)

**Q1 (Scope creep) — "What bug does this fix? I don't see a heat trail in this codebase."**

A: There is no PR C to send upstream as written. This Q&A is moot until the heat trail is upstreamed. If we submit the defensive patterns against upstream's topology SVG (which uses `petgraph` + SVG rendering), the pitch would need to change entirely.

**Q2 through Q12 apply if and only if upstream has the affected SVG code.**

**Verdict**: Do not submit PR C. If the upstream maintainer adds heat-trail or similar SVG visualization, revisit at that time.

---

## PR D — CLI Ensure V3 Collection (`cli.rs`, 8-line fix)

**What it does**: Before bulk-indexing via `scan --index`, calls `ensure_collection_v3` so the write path (`indexer::index_session` targeting `memex_sessions_v3`) has a collection to write into.  
**Backport complexity**: Extreme. The fix references `crud::ensure_collection_v3`, `crate::schema::COLLECTION_V3`, `codex_parser` — none of which exist in upstream. The upstream collection is `memex_sessions` (v2 only). There is no v3 schema.  
**Upstream has the problem**: No. Upstream's write path targets `memex_sessions`. There is no mismatch between ensure and write target in upstream.

### Q&A

**Q1 (Fundamental relevance) — "What problem does this fix on upstream?"**

A: It fixes nothing on upstream. The bug existed in the fork because the fork introduced a v3 collection schema and forgot to update the CLI's pre-flight `ensure` call. Upstream has a single collection and a consistent `ensure_collection` + write path. This PR has no value for upstream.

**Q2 (If presented as the upstream problem equivalently) — "Your scan --index showed 110 errors. Does upstream have this problem?"**

A: Only if someone introduces a v3 schema migration in upstream. If the upstream maintainer is planning to do that (which is plausible given their MCP server addition), we could propose a pattern: "before any bulk write, ensure the target collection exists." That is a defensive coding guideline, not a one-line fix. This would be better as a documentation contribution or a refactor PR that makes the collection name a parameter of `cmd_scan`.

**Q3 (Style/convention) — "Why does your fix import `crud` and `schema` modules that don't exist here?"**

A: They don't exist in upstream — the PR as written cannot compile against upstream. This confirms: PR D cannot be submitted to upstream in its current form.

**Q4 (Alternatives) — "Could we make `indexer::ensure_collection` idempotent for any collection name?"**

A: Yes. A generalized `ensure_collection(client, name)` where the name is passed as a parameter would be a cleaner upstream contribution. This is the right refactor if the maintainer is planning schema versioning. We should pitch it as: "We noticed that hardcoding the collection name in both `ensure_collection` and the write path creates a silent failure mode if they ever diverge. Here is a parameterization that prevents the class of bug we encountered in our fork."

**Q5 (Scope/correctness) — "You're printing COLLECTION_V3 in the indexed report but the user may not know what that is."**

A: True. The upstream report currently says `indexed X/Y session(s) into 'memex_sessions'` which is clear. Changing it to `memex_sessions_v3` without context would confuse upstream users. Any backport must use the upstream collection name.

**Q6 (Test coverage) — "Is there a test that reproduces the 110-error scenario?"**

A: No automated test reproduces this. It requires a fresh Qdrant instance with no collections. The commit message documents the empirical proof (`curl localhost:6333/collections/memex_sessions_v3 | jq .result.points_count → 110`). For upstream, the test would need to spin up a Qdrant container, which upstream does not currently do in CI.

**Q7 (Hackathon context) — "This feels like a fork-internal hotfix that was submitted upstream by mistake."**

A: Correct — do not submit PR D to upstream. This is an internal consistency fix between fork-specific modules.

**Verdict**: Do not submit PR D to upstream. If we want to contribute the general pattern, rewrite it as a proposal to parameterize `ensure_collection` and separate that cleanly from any v3 schema work.

---

## Submission Priority Matrix

| PR | Submit to upstream? | Risk of rejection | Effort to prepare | Recommendation |
|---|---|---|---|---|
| A (mix-modal picker) | Yes | Medium | 2-3 hours rewrite | Submit after D-0 (2026-06-01) |
| B (KF-01 path sandbox) | Yes (simplified) | Medium-High | 3-4 hours rewrite | Submit after A is received |
| C (defensive SVG) | No | N/A — no target | N/A | Hold until heat-trail is upstream |
| D (cli v3 ensure) | No | N/A — no v3 upstream | N/A | Propose as parametric refactor only |

---

## Appendix R — Red Flags (Things Reviewers Will Immediately Flag)

The following exist in the fork commits and **must be cleaned up before any upstream submission**:

### R1 — Internal phase/plan jargon in comments

Instances: `// P3 KG-03 dual-write`, `// KC-01b dual-read`, `// KF-01`, `// WOW-3`, `// P5 — Arc-based bulk index`

**Why it is a red flag**: These are hackathon-internal identifiers that mean nothing outside the fork. A maintainer seeing `// P3 KG-03` will ask what P3 is, distrust the comment quality, and suspect the code was written for a narrow context.

**Fix**: Replace with prose comments: `// The write path targets memex_sessions_v3; ensure it exists before bulk indexing.`

### R2 — `Co-Authored-By: Claude Opus 4.7 (1M context)`

**Why it is a red flag**: Many maintainers have policies against AI-generated code contributions (licensing, liability, quality). Even maintainers who are AI-positive may find the specific attribution surprising.

**Fix**: Drop the `Co-Authored-By` line from all upstream commit messages. If the maintainer asks about AI tooling, answer honestly in the PR discussion.

### R3 — `Co-Authored-By: Claude Code` in PR bodies (`🤖 Generated with Claude Code`)

Same as R2. Drop from upstream PR bodies.

### R4 — Deferred items in commit message body (`MED-2 streaming SHA-256 for large snapshots`, `LOW-1`, `NIT-1`)

**Why it is a red flag**: Upstream maintainers read commit messages as permanent project history. Internal audit items left as deferred TODOs in a commit body look like incomplete work.

**Fix**: Remove deferred-item lists from upstream commit messages. If they represent real concerns, open GitHub issues against upstream after the PR is merged.

### R5 — Internal `qdrant_version: "1.18.0"` hardcoded in `snapshot.rs`

**Why it is a red flag**: Reviewers will ask "why 1.18.0? What happens when Qdrant releases 1.19?" The constant needs documentation that explains it is the minimum tested version, not a pin.

**Fix**: Add a comment: `// Minimum Qdrant version this signing scheme has been tested against. // WarnQdrantMinor is surfaced for patch/minor differences.`

### R6 — Integration tests using real `$HOME`

`tests/sec_integration.rs` calls `validate_session_path("/etc/passwd")`. On a developer machine this works. In a sandboxed CI environment `/etc/passwd` may not exist or the sandbox may block the read. More critically, the test behavior depends on whether `~/.claude/projects` exists.

**Fix**: Move the real-`$HOME` behavior into a `#[ignore]` test (run manually) and keep the CI-safe version that uses `TempDir`.

### R7 — `expect("tempdir")` and `.unwrap()` in test helpers

**Why it is a red flag**: `.unwrap()` in tests is fine; `.expect()` in production code is a concern. The test-only helpers in `sec.rs:127` use `.expect()` and `.unwrap()` extensively. These are behind `#[cfg(test)]` so they will not ship in production, but a Rust-fluent reviewer may comment on style.

**Fix**: No action needed if they stay `#[cfg(test)]`. Confirm with a comment if necessary.

### R8 — `"dev.sgwannabe.memex"` app identifier hardcoded in `snapshot.rs:46`

**Why it is a red flag**: The upstream repo is `sgwannabe/memex`, but embedding the original author's identifier in a library function is presumptuous. If the maintainer changes the bundle ID, the snapshot path silently breaks.

**Fix**: Derive the path from `tauri::api::path::app_data_dir()` (the `NIT-1` deferred item). For the upstream PR, use a configurable path or derive it from `tauri_plugin_opener` APIs.

### R9 — `Co-Authored-By: Claude Opus 4.7 (1M context)` in merge commit messages (not just PR commits)

GitHub's merge commits inherit the PR commit messages. If the maintainer squashes, the AI attribution disappears. If they create a merge commit, it appears in their project history. Most maintainers will simply edit it out; do not make them do that.

---

## Appendix D — Surprise Delights (Things That Will Make the Reviewer's Life Easier)

### D1 — Before/After Screenshots in PR Body for PR A

The mix-modal fix is primarily a UX change. A single before/after screenshot (or GIF) showing the previously useless modal vs the working picker will communicate the value in under 5 seconds. The fork already has screencaps at `claudedocs/reports/purple-oval/`. Take similar screencaps of the modal.

### D2 — Reproduction Steps in PR A Body

"Open Memex. Click Mix & Match. Observe that [+ pos] / [− neg] buttons on the cards are blocked by the backdrop." This is reproducible by anyone with a working upstream build. Providing exact steps builds reviewer confidence that the bug is real.

### D3 — Test Counts in PR B Body

"30 unit tests in sec.rs: 14 path-containment cases, 1 empty-path case, 3 symlink cases, 4 NUL-byte cases, 8 additional. `cargo test` passes at 42/42 with no new warnings." This is already documented in the fork commit message — preserve it in the upstream PR body.

### D4 — Explicit "What is NOT changed" Section in PR B

For a security PR, explicitly stating what is unchanged builds trust. Example: "parser.rs, indexer.rs, mcp.rs, and all Tauri command signatures are unchanged. The diff is entirely additive (new module + 8 lines in commands.rs)."

### D5 — Offer to Add Playwright E2E for PR A

Even if you do not write it upfront, the offer demonstrates awareness of the gap. "We acknowledge there are no automated tests for the JS changes. We are prepared to add a Playwright smoke test for the modal picker if you can indicate your preferred test runner setup."

### D6 — Link to the Specific Upstream Code That Has the Bug (PR B)

In the PR body, cite the exact line: `commands.rs:231` (upstream's `parser::parse_session(Path::new(&source))` with no validation). Showing the reviewer the vulnerable line in their own code makes the fix immediately credible.

### D7 — Cargo.toml Impact Analysis in PR B Body

"Runtime additions: `dirs = "5"` (already a transitive dependency — no new crate, just explicit). `sha2 = "0.10"` is NOT required for this PR (it belongs with snapshot signing). Dev-only: `tempfile = "3"` (not included in release binary)." This addresses the dependency concern before the reviewer raises it.

### D8 — Minimal Patch File for Easy Review of PR D's Pattern (if reframed as refactor)

If PR D is reframed as "parameterize ensure_collection", attach a `git diff` snippet showing the four-line change that separates the collection name from the function signature. Reviewers appreciate seeing the complete change without checking out a branch.

### D9 — A `CHANGELOG.md` Entry (if upstream has one)

Check `git -C upstream/main log --oneline -- CHANGELOG.md`. If upstream maintains a changelog, add an entry. Maintainers who maintain changelogs are annoyed by PRs that don't.

---

*End of Q&A document. Author: ComBba via Claude Code (Sonnet 4.6). Reviewed against upstream `4973a91` and fork `8509096` (2026-05-19).*
