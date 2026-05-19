## Summary

The Mix & Match `<dialog>` is currently unusable on first open because its
only documented way to add positive/negative anchors is to click buttons
that live on cards **behind** the modal backdrop. This PR adds a small
search-and-pick UI inside the dialog so users can populate the drop zones
without dismissing the modal.

## Problem / Motivation

`src/index.html:87` opens `#mix-modal` via `HTMLDialogElement.showModal()`,
which paints a backdrop pseudo-element over the rest of the page. The
modal's empty-state copy (rendered by `renderMixDropzones` in `src/main.js:1418`)
reads:

> "click + pos / − neg on a card to add…"

The `+ pos` / `− neg` buttons it refers to are rendered on the stack cards
in the main view (see `src/main.js:482-493` for stack cards and
`src/main.js:734-744` for result cards). With the dialog backdrop active
those buttons receive no pointer events, so the user sees the instruction
but cannot act on it. Clicking `Run discovery` then returns the inline
error `"Add at least one positive or negative session first."`
(`src/main.js:1445-1448`) — a dead-end loop.

### Reproduction

1. `npm run tauri dev`
2. Index at least one session so the stack has cards.
3. Click the "Mix & Match" button in the top bar.
4. Observe the modal's empty-state message.
5. Try to click any `+ pos` button on a card behind the backdrop —
   nothing happens.
6. Click `Run discovery` — see the dead-end error.
7. The only way to escape today is to close the modal, click
   `+ pos` on a card, then re-open the modal.

## Solution

Add a self-contained picker inside `#mix-modal`. The picker delegates to
the existing `addToMix(side, sessionId)` flow, so all downstream logic
(drop-zone rendering, `mix_match` invocation, results rendering) is
unchanged.

Changes by file:

- **`src/index.html`** — new `.mix-picker` section between the existing
  `<p class="side-desc">` and the drop-zone area. Input + results list.
  The misleading `"drop ids here…"` hint (drag-and-drop was never
  implemented) is updated to mention both real paths.
- **`src/main.js`** — four new functions wired into the modal lifecycle:
  - `runMixPickerSearch()` — calls the existing `lens_search` Tauri
    command with `state.weights` and `limit: 12`. A UUID-shaped input
    short-circuits to a direct pick with no Qdrant round-trip.
  - `renderMixPickerRow(hit, parent)` — builds one row using DOM
    `createElement` + `textContent` throughout (no `innerHTML` for
    user-derived data) with `[+ pos]` / `[− neg]` buttons that flip to
    `✓ pos` / `✓ neg` after the action.
  - `attachMixPickerEvents()` — Enter-to-search + 350 ms-debounced
    input (auto-search after 2+ chars).
  - `updateRunMixButton()` — disables `[Run discovery]` with an
    explanatory `title` tooltip until at least one anchor is staged.
  - `addToMix` / `removeFromMix` get one-line hooks to keep button
    state in sync.
- **`src/styles.css`** — `.mix-picker*` selectors (~110 LOC) plus a
  global `.btn[disabled]` rule. Uses `oklch()` for consistency with the
  picker's own palette — happy to rewrite to existing CSS custom
  properties if preferred. (Tauri 2 target WebViews ship Safari 15.4+ /
  Chromium 111+, both of which support `oklch()`.)

The main-view stack-card `[+ pos]` / `[− neg]` buttons still work and
still dispatch through the same `addToMix` path for users who prefer to
pre-stage selections before opening the modal.

## Trade-offs / Alternatives considered

- **Move the backdrop / make the dialog non-modal.** Rejected: the
  modal's spatial focus is part of the UX, and a non-modal would lose
  `Esc`-to-close and inert-background semantics.
- **Bind `pointer-events: auto` on the stack cards while the dialog is
  open.** Rejected: fights browser semantics, conflicts with future
  `inert` migration, and visually misleading (cards look enabled but the
  backdrop greys them).
- **Just change the hint copy** to "close this dialog, add cards, then
  reopen." Rejected: that's a UX regression, not a fix.

## Behavioral changes beyond the bug fix (call-outs)

1. `<p class="side-desc">` rewritten to plain-English (was "drag two or
   more results below…" — but drag-and-drop was never implemented).
2. Dropzone hint copy updated (the old "drop ids here…" was misleading).
3. `[Run discovery]` is now disabled with an explanatory tooltip until
   at least one anchor exists (previously enabled-but-erroring).

No telemetry, no API, no schema, no preference, no keybinding changes.

## Test plan

Manual smoke (single-window Tauri):

- [ ] `npm run tauri dev` boots without console errors.
- [ ] Mix & Match modal opens; the new picker is visible above the
      drop zones; `[Run discovery]` is disabled with a tooltip.
- [ ] Typing a query and pressing `↵` renders up to 12 rows.
- [ ] Typing 2+ chars and pausing also auto-searches (350 ms debounce).
- [ ] Pasting a 36-char UUID with no spaces treats it as a direct
      session pick (no `Searching…` shown, no `lens_search` call).
- [ ] `[+ pos]` button flips to `✓ pos`, disables, and the chip appears
      in the positive drop zone; `[Run discovery]` becomes enabled.
- [ ] Clicking `[Run discovery]` produces results in `#mix-results` as
      in the existing flow.
- [ ] Removing the chip from the drop zone re-runs the picker search
      and the corresponding row's button returns to `+ pos`.
- [ ] Closing and re-opening the modal preserves no stale picker state
      (input clears, results clear).
- [ ] The original stack-card `+ pos` / `− neg` path still works for
      users who stage selections before opening the modal.

## Backwards compatibility

No backend changes. No Tauri command surface change. No `state` shape
change. The existing stack-card path (`renderStackCard` → `addToMix`) is
unchanged. Existing keyboard shortcuts are untouched. `cargo check`
passes with no Rust changes.

## Security

Picker rows are built with `document.createElement` + `textContent`
throughout (not `innerHTML` with `escapeHtml` interpolation), so untrusted
Qdrant payload fields (`project_name`, `ai_title`, `start_iso`) cannot be
interpreted as markup — even if a future code path forgets to escape.
The `session_id` is used only as a string argument to `addToMix` and is
never rendered as HTML. Search errors are also rendered via
`textContent`.

## Screenshots

I'm happy to attach before/after captures on request — the modal
dead-end is reproducible on `main` by following the steps above.

## Out of scope (intentionally)

- Keyboard arrow-key navigation between picker rows (matches the
  existing main-view card pattern — this PR does not widen the gap).
- Drag-and-drop (never implemented; the old `"drop ids here…"` copy was
  aspirational).
- Any change to `lens_search` / `mix_match` command surfaces.
- Automated frontend tests (upstream has no JS test runner configured).
  Happy to add a Playwright smoke test for the modal picker if you can
  indicate your preferred testing setup.

## Co-authorship / attribution

Originally explored in fork `ComBba/memex@e402b1f`. Re-implemented for
upstream compatibility in this PR: removes fork-only dependencies
(`lens_search_v2`, `state.mix.target`, hyperplane canvas,
`mix-dropzones` grid, self-priming-on-open) and tightens the XSS
posture (DOM construction instead of `innerHTML` + `escapeHtml`).
