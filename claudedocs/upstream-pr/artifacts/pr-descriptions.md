# Upstream PR descriptions — drafts for `sgwannabe/memex`

> **Status**: Drafts. Each PR's *ship-ready* gate is decided by the other
> agents producing files under `claudedocs/upstream-pr/candidates/` and
> `claudedocs/upstream-pr/reviews/`. Once those land, revisit the **Test
> plan** section of the matching PR below and replace the placeholder
> with a link to the agent-produced `test-plan.md`.
>
> All four drafts assume the contributor has rebased a clean topic branch
> onto `upstream/main` and that the changes have been **re-implemented**
> against upstream code (no fork-only modules such as `codex_parser.rs`,
> `lens.rs`, `heat-trail` JS/CSS are referenced).

---

## PR A — Mix & Match modal: self-contained picker

### Title

```
fix(ui): make Mix & Match modal self-contained (search + add inside dialog)
```

(67 chars)

### Body

```markdown
## Summary

The Mix & Match `<dialog>` is currently unusable on first open because its
only documented way to add positive/negative anchors is to click buttons
that live on cards **behind** the modal backdrop. This PR adds a small
search-and-pick UI inside the dialog so users can populate the drop zones
without dismissing the modal.

## Problem / Motivation

`src/index.html:87` opens `#mix-modal` via `HTMLDialogElement.showModal()`,
which paints a backdrop pseudo-element over the rest of the page. The
modal's empty-state copy reads:

> "Click `+ pos` / `− neg` on a card to add at least one positive or
> negative session, then press Run discovery."

The `+ pos` / `− neg` buttons it refers to are rendered on the stack cards
in the main view (see `src/main.js` — the `renderStackCard()` path). With
the dialog backdrop active those buttons receive no pointer events, so
the user sees the instruction but cannot act on it.

### Reproduction

1. `npm run tauri dev`
2. Index at least one session so the stack has cards.
3. Click the "Mix & Match" button (or trigger whatever opens
   `#mix-modal`).
4. Observe the modal's empty-state message.
5. Try to click any `+ pos` button on a card behind the backdrop —
   nothing happens.
6. The only way to escape the dead-end is to close the modal, click
   `+ pos` on a card, then re-open the modal.

## Solution

Add a self-contained picker inside `#mix-modal` so users can search and
add anchors without leaving the dialog. The picker delegates to the
existing `addToMix(side, sessionId)` flow, so all downstream logic
(drop-zone rendering, `run_mix` invocation, results rendering) is
unchanged.

Changes by file (against `upstream/main`):

- **`src/index.html`** — new `.mix-picker` section between the existing
  modal header and the drop-zone area. ~35 lines of markup: input,
  results list, hint text.
- **`src/main.js`** — three new functions wired into the modal lifecycle:
  - `runMixPickerSearch(query)` — calls `lens_search` (or whichever
    search command exists upstream) and renders up to 12 rows. A pure
    UUID input short-circuits to a direct pick.
  - `renderMixPickerRow(hit)` — renders a row with `[+ pos]` / `[− neg]`
    buttons that flip to `✓ pos` / `✓ neg` after the action.
  - `attachMixPickerEvents()` — debounced input, Enter-to-search, and
    state-sync hooks invoked from `addToMix` / `removeFromMix` so picker
    rows stay in sync with the drop zones.
- **`src/styles.css`** — `.mix-picker*` selectors (~110 LOC) plus a
  global `.btn[disabled]` rule. The Run-discovery button is disabled
  with an explanatory `title` until at least one anchor is picked.

The original stack-card `+ pos` / `− neg` buttons remain unchanged for
users who prefer the pre-stage flow.

## Trade-offs / Alternatives considered

- **Move the backdrop / make the dialog non-modal.** Rejected: the
  modal's spatial focus is part of the UX, and a non-modal would lose
  the `Esc`-to-close and inert-background semantics.
- **Add a "minimize modal" toggle.** Rejected: heavier UX, doesn't solve
  the first-time discoverability problem.
- **Bind `pointer-events: auto` on the stack cards while the dialog is
  open.** Rejected: fights browser semantics, conflicts with future
  `inert` migration, and visually misleading (cards look enabled but the
  backdrop greys them).

## Test plan

> _A formal manual test plan will be linked here once
> `claudedocs/upstream-pr/candidates/pr-a/test-plan.md` is finalized._

Manual smoke (single-window Tauri):

- [ ] `npm run tauri dev` boots without console errors.
- [ ] Mix & Match modal opens; the new picker is visible above the drop
      zones.
- [ ] Typing a query and pressing Enter renders rows.
- [ ] Pasting a 36-char UUID with no spaces treats it as a direct
      session pick (no search call).
- [ ] `[+ pos]` button flips to `✓ pos` and the chip appears in the
      positive drop zone.
- [ ] Removing the chip from the drop zone re-enables the picker row's
      button.
- [ ] `Run discovery` is disabled with a tooltip until at least one
      anchor exists; clicking it after a pick produces results.
- [ ] Closing and re-opening the modal preserves no stale picker state.
- [ ] The original stack-card `+ pos` / `− neg` path still works for
      users who stage selections before opening the modal.

## Backwards compatibility

No backend changes. No Tauri command surface change. The existing
stack-card path (`renderStackCard` → `addToMix`) is unchanged. Existing
keyboard shortcuts and the `state` shape are untouched.

## Screenshots / recordings

Please attach to the PR before review:

- [ ] **Before**: screencap of the modal with the dead-end message, plus
      one of a failed click on a `+ pos` button behind the backdrop
      (devtools showing the click was eaten).
- [ ] **After**: screencap of the picker populated with results.
- [ ] **After**: short clip (≤15s) of search → `+ pos` → `Run discovery`
      → results.

## Co-authorship / attribution

Original implementation explored in fork `ComBba/memex` commit
`e402b1f`. Re-implemented for upstream compatibility in this PR.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Commit message

```
fix(ui): make Mix & Match modal self-contained (search + add inside dialog)

The Mix & Match <dialog> is opened with showModal(), which paints a
backdrop over the rest of the page. The only documented way to populate
the drop zones was to click `+ pos` / `− neg` buttons that live on the
stack cards behind the backdrop. Those clicks are eaten by the backdrop,
leaving the modal in a permanent dead-end on first open.

This change adds a small search-and-pick UI inside the modal:

  - new .mix-picker section in src/index.html
  - runMixPickerSearch / renderMixPickerRow / attachMixPickerEvents in
    src/main.js
  - .mix-picker* + .btn[disabled] rules in src/styles.css

Picker rows delegate to the existing addToMix() path, so the rest of the
discovery flow (drop zones, run_mix, results render) is unchanged. The
stack-card buttons still work for users who pre-stage selections.

No backend or Tauri command changes.

Signed-off-by: <contributor>
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Suggested labels

`bug`, `ui`, `ux`, `frontend`

### Reviewer suggestions

- **Frontend / UX reviewer** — has opinions on dialog semantics,
  `inert`, focus management, and the existing Mix & Match flow.
- Familiarity with `lens_search` / search command surface is helpful but
  not required.

---

## PR B — KF-01: path sandbox for session JSONL reads

### Title

```
feat(security): sandbox session-file reads under known agent roots
```

(64 chars)

### Body

```markdown
## Summary

Add a small `SandboxRoot` validator that constrains every session-JSONL
read to an explicit allow-list of directories (currently
`~/.claude/projects`). All other paths — including symlinks pointing
outside the root and `..` traversal — are rejected before
`parser::parse_session` is invoked.

## Problem / Motivation

`src-tauri/src/commands.rs:174` (`get_session_turns`) and the
`predict_next_actions` neighbour-walk inside `indexer.rs` currently call
`parser::parse_session(path)` with whatever `PathBuf` the caller
supplies. There is no defense against:

- A frontend bug or compromised IPC sender passing an arbitrary path
  (`/etc/passwd`, `~/.ssh/id_rsa`).
- A symlink inside the legitimate session root that resolves to a
  sensitive file elsewhere on disk.
- Path strings containing embedded NUL bytes that Rust's `Path` accepts
  but that downstream OS calls truncate.

This is a defense-in-depth gap. Memex is a local desktop app, but the
Tauri IPC surface still benefits from validating untrusted-string-style
inputs at the boundary.

### Reproduction (illustrative)

A malicious or buggy frontend invokes:

```ts
invoke("get_session_turns", { path: "/etc/passwd" });
```

Today this hits `parse_session` directly; the function fails on parse
errors but the file *was* opened and read. After this PR the request is
rejected at the boundary with a clear error.

## Solution

Add `src-tauri/src/sec.rs`:

- `SandboxRoot` — owns a canonicalized allow-listed directory.
  Construction is **graceful**: if `~/.claude/projects` does not exist
  yet (fresh install) the constructor returns `None` rather than
  erroring.
- `validate_session_path(p: &Path) -> Result<PathBuf>` — rejects:
  - Empty paths
  - Paths containing a NUL byte (checked on the OS-string bytes, not
    just `to_str()`)
  - Paths that fail `canonicalize`
  - Canonical paths that are not a descendant of the sandbox root
  - Symlinks whose target escapes the root (validated by canonicalizing
    both pre- and post-resolution)
- Returns the canonicalized path on success.

Wire-in:

- `commands::get_session_turns` (`src-tauri/src/commands.rs:174`):
  `validate_session_path` → `parse_session`.
- `indexer::predict_next_actions`: validate the active `session_path`
  and each neighbour's `session_path` before parsing. Invalid
  neighbours are skipped rather than failing the whole prediction.

14 unit tests in `sec.rs` cover:

- empty input, NUL bytes, `..` traversal, missing file
- symlink pointing inside the root (accept)
- symlink pointing outside the root (reject)
- arbitrary random-bytes paths (no panic)
- relative paths that canonicalize inside the root (accept)

Plus integration tests in `tests/sec_integration.rs` exercising the
full command path with `tempfile`.

## Trade-offs / Alternatives considered

- **Use the OS file-dialog as the only entry point.** Doesn't cover
  programmatic IPC callers (`get_session_turns` is invoked from JS, not
  from a file picker).
- **Check the path string with a prefix match (no canonicalize).**
  Rejected: trivially bypassable with symlinks or `..`.
- **Use a third-party crate (e.g. `cap-std`).** Considered, but
  `cap-std` is a heavier dependency and the validation surface here is
  small enough that ~280 LOC of std-only Rust is easier to audit.
- **Make the root configurable.** Deferred — the upstream code today
  has a single hard-coded root, so a fixed-root sandbox matches current
  expectations. A future PR could add a `sandbox.toml`.

## Test plan

> _A formal cargo test plan will be linked here once
> `claudedocs/upstream-pr/candidates/pr-b/test-plan.md` is finalized._

- [ ] `cargo test -p memex --lib sec::` — 14 unit tests pass
- [ ] `cargo test -p memex --test sec_integration` — integration tests
      pass
- [ ] `cargo test -p memex --release` — full suite (existing tests
      unchanged)
- [ ] `cargo clippy -p memex -- -D warnings`
- [ ] Manual: open the app, browse a real session — works
- [ ] Manual: with devtools, call `invoke("get_session_turns", { path:
      "/etc/passwd" })` — receives a clear sandbox error string
- [ ] Manual: same with a symlink inside `~/.claude/projects` pointing
      to `/etc/hosts` — rejected
- [ ] Manual: fresh user with no `~/.claude/projects` — app still boots,
      the sandbox is just absent (commands return a clear "no sandbox
      configured" error rather than panicking)

## Backwards compatibility

- Path strings that **were** valid (real files inside
  `~/.claude/projects`) remain valid. The function returns the
  canonicalized form, which is what `parse_session` would have used
  anyway.
- Fresh installs with no `~/.claude/projects` directory continue to
  boot. Reads against the missing root return an explicit error
  instead of an opaque `parse_session` failure — this is a string
  change visible to the frontend but no schema change.
- One new runtime dep (`sha2`) and one dev dep (`tempfile`). These are
  small and widely used; both are already in the Cargo dependency
  graph through transitive deps in most builds.

## Screenshots / recordings

This is a backend change; no UI screenshots. Please attach:

- [ ] `cargo test` output showing the new tests passing
- [ ] A short clip of devtools showing a rejected `/etc/passwd` request
      with the new error string

## Co-authorship / attribution

Originally implemented as part of a larger security pass in fork
`ComBba/memex` commit `f55d417`. This PR extracts only the path-sandbox
piece (`KF-01`) so it can be reviewed independently of the snapshot
sandbox and signed-envelope work (which would warrant separate PRs).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Commit message

```
feat(security): sandbox session-file reads under known agent roots

Adds src-tauri/src/sec.rs with a SandboxRoot validator that constrains
every session-JSONL read to an explicit allow-listed directory
(~/.claude/projects). The validator rejects:

  - empty paths and paths containing NUL bytes
  - paths that fail canonicalize
  - canonical paths that escape the root (incl. via symlink)
  - .. traversal pre- and post-canonicalize

Wired into:
  - commands::get_session_turns (commands.rs:174)
  - indexer::predict_next_actions (active + each neighbour)

14 unit tests in sec.rs and integration tests in
tests/sec_integration.rs cover NUL bytes, symlink-in vs symlink-out,
.. traversal, missing files, and arbitrary-bytes inputs.

Graceful on fresh installs: if ~/.claude/projects is absent, the
sandbox is None and the affected commands return an explicit error
rather than panicking.

No schema change. No frontend command surface change beyond the new
error string on rejected paths.

Signed-off-by: <contributor>
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Suggested labels

`security`, `enhancement`, `backend`, `rust`

### Reviewer suggestions

- **Security-minded reviewer** — interested in path-handling correctness,
  symlink behaviour, and Tauri IPC trust boundaries.
- **Rust reviewer** familiar with `std::path::Path::canonicalize` quirks
  on macOS (e.g. `/var` → `/private/var`) and with the existing
  `commands.rs` IPC surface.

---

## PR C — Defensive primitives for SVG stroke rendering

### Title

```
fix(svg): clamp scores and pin stroke pixels in SVG overlay rendering
```

(66 chars)

### Body

> **Note to maintainer:** this PR is only meaningful if upstream contains
> an SVG overlay whose stroke width is derived from a similarity score
> coming out of `lens_search` (or equivalent). The bug we hit on the
> fork manifested in a feature (`heat-trail`) that does not exist on
> upstream. **If upstream has no such overlay, please close this PR.**
> The patterns documented below may still be useful as a checklist for
> any future score-driven SVG work.

```markdown
## Summary

When SVG strokes are sized from a similarity score, three small bugs can
combine to produce a viewport-spanning artefact:

1. The score is treated as if it were a cosine similarity in `[0, 1]`,
   but multi-vector fusion scores can exceed `1`.
2. The SVG `viewBox` is scaled to fit a parent container, which
   multiplies user-space stroke widths.
3. `stroke-linecap: round` turns the endpoints of a thick stroke into
   half-circles whose diameter equals the stroke width.

This PR proposes a small set of defensive primitives any future
score-driven SVG can adopt to prevent the failure mode.

## Problem / Motivation

In score-driven SVG rendering, the natural code looks like:

```js
path.setAttribute("stroke-width", 1.6 + Math.max(0, score) * 2.4);
```

If `score` is assumed to be in `[0, 1]` (a cosine similarity) the
upper bound is ~4 user-space units. If it is actually the output of
`lens_search` (a weighted multi-vector fusion that, with default slider
weights, easily reaches `3–6`), the stroke can be 9–16 user-space
units. On a `viewBox` that is scaled up to fit a wider container, and
with `stroke-linecap: round`, the result is a vertical capsule that can
span the visible area.

We hit this on the fork; the root-cause analysis is in
`ComBba/memex@deed283`. If upstream introduces a similar overlay, the
same trap is one careless line away.

## Solution

Three independent guards, applied at the point each stroke attribute is
set:

1. **Clamp the visual score**: introduce `clampUnit(score)` that returns
   `Number.isFinite(score) ? Math.max(0, Math.min(1, score)) : 0`. The
   raw fusion score is still preserved on the model for any breakdown
   panel.
2. **Absolute pixel cap**:
   `stroke-width = Math.min(CAP_PX, base + clampUnit(score) * SLOPE)`.
   `CAP_PX = 4.5` works well for the existing visual language.
3. **Pin stroke pixels through `viewBox` scaling**:
   `el.setAttribute("vector-effect", "non-scaling-stroke")` on the
   path *and* on any endpoint marker. This makes the stroke render in
   screen pixels regardless of the `viewBox` → CSS box ratio.

CSS belt-and-suspenders:

```css
.heat-trail path,
.heat-trail circle {
  stroke-width: min(4.5px, var(--trail-w));
}
```

Plus a viewBox sanity gate: if the overlay's container is smaller than
~100×100 (e.g., not laid out yet on first paint), bail out and let the
next animation frame retry — a near-zero viewBox is the worst possible
multiplier.

## Trade-offs / Alternatives considered

- **Normalize the score in the search backend.** Cleaner in the long
  run, but it is a behaviour change for any caller that consumes the
  fused score (UI sliders, dashboards). The frontend clamp is purely
  presentational.
- **Use CSS `clamp()` only.** Insufficient on its own: the user-space
  stroke is multiplied by the `viewBox` scale **before** the CSS
  cascade. `vector-effect: non-scaling-stroke` is the part that
  actually breaks the scale dependency.
- **Hard-code `preserveAspectRatio="none"`.** Considered for the
  viewBox-to-CSS map, but it warps anything else inside the SVG. Only
  apply if the overlay contains nothing but score-driven strokes.

## Test plan

> _A formal manual test plan will be linked here once
> `claudedocs/upstream-pr/candidates/pr-c/test-plan.md` is finalized._

- [ ] Hover/trigger the overlay with a search that returns scores `>1`.
      Stroke width never exceeds ~4.5 CSS px regardless of score.
- [ ] Resize the window from narrow to ultra-wide. Stroke width is
      pixel-stable.
- [ ] Inject `Infinity` and `NaN` as score values via devtools — neither
      crashes nor renders.
- [ ] Trigger the overlay before layout (e.g. immediately on app boot).
      It either skips that frame or renders correctly; it does not
      produce a giant artefact.
- [ ] No regression in the intended thin-curve look for normal `[0, 1]`
      scores.

## Backwards compatibility

Purely presentational. The raw `score` field on result objects is
unchanged. Any downstream UI that surfaces the score for explanation
purposes (breakdown panels, sliders) sees the same number it sees
today.

## Screenshots / recordings

- [ ] **Before**: screencap of the viewport-spanning artefact, plus a
      DevTools panel showing the computed stroke width on the offending
      `<path>`.
- [ ] **After**: screencap of the intended thin-curve overlay with the
      same input.

## Co-authorship / attribution

Root-cause analysis and the defensive pattern set were developed in
fork `ComBba/memex` commit `deed283`. This PR is generalized for
upstream — no `heat-trail`-specific code is included.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Commit message

```
fix(svg): clamp scores and pin stroke pixels in SVG overlay rendering

When an SVG stroke is sized from a similarity score, three small bugs
combine into a viewport-spanning artefact:

  1. Score treated as cosine [0, 1] but lens_search returns a weighted
     fusion sum that routinely exceeds 1.
  2. SVG viewBox scaled to fit its container, multiplying user-space
     stroke widths.
  3. stroke-linecap: round expands each endpoint into a half-circle
     whose diameter equals stroke width.

This change adds three orthogonal guards:

  - clampUnit(score) forces the visual score into [0, 1] while
    preserving the raw score on the model.
  - stroke-width is capped at a per-overlay absolute pixel value
    (CAP_PX) via Math.min.
  - vector-effect="non-scaling-stroke" is set on every stroked element
    so the stroke renders in screen pixels regardless of viewBox
    scaling.

Plus a viewBox sanity gate that skips the frame when the container is
<100x100 (not yet laid out), and a CSS belt-and-suspenders min() cap.

No change to the raw score field or to any backend.

Signed-off-by: <contributor>
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Suggested labels

`bug`, `ui`, `frontend`, `svg`

### Reviewer suggestions

- **Frontend reviewer** comfortable with SVG attribute semantics
  (`vector-effect`, `viewBox`, `preserveAspectRatio`,
  `stroke-linecap`).
- Helpful but not required: familiarity with `lens_search` score
  semantics so they can confirm the `[0, 1]` assumption was wrong.

---

## PR D — CLI: ensure target collection before bulk index

### Title

```
fix(cli): ensure target collection exists before scan --index
```

(58 chars)

### Body

> **Note to maintainer:** the fork carries a `v3` schema that upstream
> does not. The fork bug existed because the CLI provisioned the legacy
> collection while the writer targeted the new one. **On upstream, where
> there is only one collection (`memex_sessions`), this exact bug does
> not reproduce** — `scan --index` already provisions that collection
> via `ensure_collection`. The PR below is offered as a small
> defensive cleanup, not a fix; please close if you'd rather not carry
> the extra check.

```markdown
## Summary

Make `memex scan --index` validate up front that the collection
`indexer::index_session` writes to actually exists, instead of relying
solely on `ensure_collection` being kept in sync with `COLLECTION`. The
check is a no-op on a healthy install but turns silent per-session
upsert failures into an explicit, single error.

## Problem / Motivation

`src-tauri/src/cli.rs:128` dispatches `scan --index` to a routine that:

1. Calls `ensure_collection(&client).await?` (creates the legacy
   collection if missing).
2. For each parsed session, calls
   `indexer::index_session(&client, session)` — which upserts into the
   constant `indexer::COLLECTION`.

Today both code paths target the same collection name, so the call
sequence is correct. The fragility is structural: if anyone changes
`COLLECTION` (e.g. for a schema migration) without also updating
`ensure_collection`, the scan will produce a long stream of per-session
gRPC errors that the CLI aggregates into a single misleading line:

```
indexed 0/N session(s) into 'memex_sessions'
  (1 duplicate sessionId(s) skipped, N error(s))
```

We hit this on the fork after introducing a `v3` schema — the CLI was
still provisioning `v2` while the writer wrote to `v3`. The fix in
fork commit `e1c075b` was to point the CLI at the same collection name
the writer uses.

## Solution

Two-line change in `src-tauri/src/cli.rs::cmd_scan` (around the call to
`ensure_collection`):

- After provisioning, call a small helper
  `indexer::ensure_target_collection(&client).await?` that:
  - Uses the same `COLLECTION` constant `index_session` consumes.
  - Returns `Ok(())` if the collection exists (the common case after
    `ensure_collection`).
  - Returns a clear error like:

    ```
    cli/index: target collection '{COLLECTION}' is missing after
    ensure_collection(); writer and provisioner are out of sync.
    ```

This guarantees the contract "if `scan --index` returned `Ok`, then the
upsert target exists" rather than discovering the mismatch one session
at a time.

## Trade-offs / Alternatives considered

- **Make `ensure_collection` take a `&str` parameter and call it with
  `COLLECTION` directly.** Cleaner long-term refactor, but expands the
  diff and touches a stable function signature. Worth doing in a
  follow-up.
- **Do nothing; the bug doesn't exist on upstream today.** Reasonable.
  The PR is offered because the failure mode is silent and very
  expensive to debug.
- **Remove `ensure_collection` and let `index_session` create the
  collection on first write.** Rejected: per-session creation races
  badly under concurrent scans.

## Test plan

> _A formal cargo test plan will be linked here once
> `claudedocs/upstream-pr/candidates/pr-d/test-plan.md` is finalized._

- [ ] `cargo test -p memex --release` — existing tests pass
- [ ] Manual: drop the `memex_sessions` collection via the Qdrant REST
      API, run `memex scan --index ~/.claude/projects --limit 10` — the
      command provisions and indexes normally.
- [ ] Manual: temporarily change `COLLECTION` to a different string
      *only* in `index_session`, leave `ensure_collection` pointing at
      `memex_sessions`. The CLI now fails fast with the new error
      message instead of N silent upsert errors.
- [ ] Manual: with a healthy install (collection already present), the
      check is a no-op and adds no observable latency.

## Backwards compatibility

No CLI surface change, no schema change, no API change. The check is
inert on every currently shipping configuration; it only affects an
error path that today silently misreports success.

## Screenshots / recordings

- [ ] CLI output before/after, under the "writer and provisioner out of
      sync" scenario, to demonstrate the error becomes obvious instead
      of silent.

## Co-authorship / attribution

The underlying failure mode was originally found while developing a
multi-collection schema in fork `ComBba/memex` (commit `e1c075b`). The
fork's fix was to align the names; this PR generalizes the contract by
making the missing-target case an explicit, single error.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Commit message

```
fix(cli): ensure target collection exists before scan --index

After ensure_collection() provisions the legacy collection, validate
that the collection index_session() actually writes to (the COLLECTION
constant in indexer) exists. On a healthy install this is a no-op; on a
configuration where the provisioner and writer have drifted apart, the
CLI now fails with a single explicit error instead of producing N
silent per-session upsert failures aggregated into a misleading
"indexed 0/N" line.

No CLI surface change, no schema change.

Signed-off-by: <contributor>
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

### Suggested labels

`bug`, `cli`, `backend`, `rust`, `defensive`

### Reviewer suggestions

- **Rust / backend reviewer** familiar with the indexer and the Qdrant
  client wrapper, especially the `COLLECTION` constant and how
  `ensure_collection` is invoked.
- A maintainer who can confirm whether the explicit safety net is
  desired upstream or whether the implicit contract is preferred.

---

## Cross-PR notes for the contributor

- All four PRs assume a **rebased topic branch onto `upstream/main`**,
  one branch per PR (`backport/mix-modal-self-contained-picker`,
  `feature/sec-path-sandbox`, `fix/svg-stroke-defensive`,
  `fix/cli-ensure-target-collection`).
- Use `git commit -s` so the `Signed-off-by` trailer in each commit
  message is real (replace `<contributor>` with the submitter's
  identity).
- The PRs are intentionally **not interdependent**: each can be
  rejected, deferred, or rewritten without affecting the others.
- Co-authorship attribution to "Claude Opus 4.7 (1M context)" reflects
  AI-assisted authorship and is preserved per the contributor's
  policy. If the upstream maintainer prefers a different convention,
  drop the trailer before opening the PR — the substantive changes do
  not depend on it.
