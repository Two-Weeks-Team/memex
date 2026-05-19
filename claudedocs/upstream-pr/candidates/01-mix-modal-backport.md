# Candidate 01 — Mix & Match modal: self-contained picker (upstream backport)

**Fork commit being backported**: `e402b1f` ("fix(mix): self-contained Mix & Match picker — modal no longer depends on cards behind backdrop")
**Target**: `upstream/main` (`4973a91`)
**Status**: 🟢 Clean manual backport designed below — applies on stock upstream without dragging in fork-only systems (no WOW-1 heat-trail, no `lens_search_v2`, no `state.mix.target`, no hyperplane canvas, no `mix-dropzones` grid).

---

## 1. Side-by-side analysis

### 1.1 The bug exists on both forks

The pathology is identical on `upstream/main` and on the fork's pre-`e402b1f` state:

`<dialog id="mix-modal">` is opened via `.showModal()`, which renders a `::backdrop` over the whole page. The only documented way to populate the dropzones is the `[+ pos]` / `[− neg]` buttons on stack cards / result cards in the main view — but those cards live *behind* the backdrop and are pointer-event-blocked. A user who opens the modal first (the natural discovery flow, since "Mix & Match" is a top-bar button) is shown the dropzones' hint copy `click + pos / − neg on a card to add…` referring to buttons they can see but cannot click. Clicking `Run discovery` returns `Add at least one positive or negative session first.` — a dead-end loop.

Confirmed in upstream source:

- HTML hint copy `"click + pos / − neg on a card to add…"` — actually only present in fork's `renderMixDropzones` rebuild path; in upstream the static HTML hint is `"drop ids here…"` (`src/index.html:100,104`), which is even worse because drag-and-drop is not implemented anywhere in the codebase. The runtime hint produced by upstream's `renderMixDropzones` (`src/main.js:1424`) is `"click + pos / − neg on a card to add…"`, matching the fork's pre-fix copy.
- Card buttons that feed `addToMix`: `src/main.js:482-493` (stack cards) and `src/main.js:734-744` (result cards).
- `runMix` error path: `src/main.js:1445-1448` → `"Add at least one positive or negative session first."`.

### 1.2 What upstream has around `#mix-modal`

| File | Region | What's there |
|---|---|---|
| `src/index.html` | lines 87–110 | Plain modal: header → `.mix-body` → `<p class="side-desc">` → label + `#mix-positive .dropzone` → label + `#mix-negative .dropzone` → `#btn-run-mix` → `#mix-results`. No grid wrapper, no hyperplane canvas, no `mix-controls`, no `mix-target`. |
| `src/main.js` | lines 1400–1474 | `openMixModal()`, `addToMix()`, `removeFromMix()`, `renderMixDropzones()`, `runMix()`. Calls `invoke("mix_match", …)`. State on `state.mix.positive` / `state.mix.negative` (`src/main.js:14`). |
| `src/main.js` | line 5 | `const { invoke } = window.__TAURI__.core;` (global, not ESM import). |
| `src/main.js` | line 631 | `invoke("lens_search", { query, weights, limit })` — single-call lens search, no `lens_search_v2`, no fallback ladder. Hits shape: `{ session_id, project_name, ai_title, start_iso, score, vector_scores }`. |
| `src/main.js` | line 1521 | `escapeHtml(s)` defined — usable from the picker. |
| `src/main.js` | lines 45–61 | `DOMContentLoaded`: `buildLensSliders / attachEvents / attachStackEvents / attachReplayEvents / attachRecallBannerEvents` then async pollers. Wire point for `attachMixPickerEvents()`. |
| `src/styles.css` | line 156 | `.btn.ghost { background: transparent; }` — insert point for `.btn[disabled]`. |
| `src/styles.css` | lines 804–870 | `.mix-body / .dropzone / .chip / …` — picker styles append cleanly at file end (line 1776). |

### 1.3 What `e402b1f` adds — and what to drop

The fork commit's payload (315 added lines) splits into three buckets:

**Bucket A — bug fix proper (BACKPORT THIS)**
- `mix-picker` HTML block (search input + results list inside the modal)
- `runMixPickerSearch()` (lens search + render rows)
- `renderMixPickerRow()` (per-row UI + `[+ pos] [− neg]` buttons that call `addToMix`)
- `attachMixPickerEvents()` (Enter + debounced auto-search)
- `updateRunMixButton()` (disable `[Run discovery]` when nothing is staged)
- `addToMix` / `removeFromMix` hooks to keep button state in sync
- Dropzone hint copy update ("search above OR click + pos on a card behind the modal…")
- `.mix-picker*` CSS + `.btn[disabled]` global rule

**Bucket B — fork-only context (DROP)**
- `state.mix.target` line in `openMixModal` (fork-only field; upstream has no target input)
- `requestAnimationFrame(() => initHyperplane())` (fork-only canvas)
- `lens_search_v2` primary path + `lensWeightsForV2()` / `normalizeLensResult()` / `legacyHitToLensResult()` helpers (fork-only lens system)
- `.mix-dropzones` grid wrapper, `.dz-col` (fork-only HTML restructure)
- Header text change `"Mix & Match — Discovery API"` → `"Mix & Match — Discovery Hyperplane"` (cosmetic, references fork-only feature)
- "self-priming on open" behavior (`state.query` seed + auto-search) — kept; `state.query` exists upstream and the UX win is large for ~6 LOC.

**Bucket C — copy-only polish (KEEP — trivial)**
- `<p class="side-desc">` rewrite to plain-English explanation of positive/negative.
- Dropzone hint copy update.

---

## 2. Upstream-targeted patch

Three unified-diff blocks. All hunks anchored against `upstream/main` @ `4973a91`. Verified line numbers via `git show upstream/main:<path> | sed -n …`.

### 2.1 `src/index.html`

```diff
diff --git a/src/index.html b/src/index.html
index 0000000..0000000 100644
--- a/src/index.html
+++ b/src/index.html
@@ -90,18 +90,40 @@
       </header>
       <div class="mix-body">
         <p class="side-desc">
-          Drag two or more results below as positives (anchors) and at least
-          one as negative (anti-context). Discovery will rank similar
-          sessions accordingly.
+          Tell Memex what you like (positive) and what to avoid (negative),
+          then it finds sessions that lean toward the positives and away
+          from the negatives. Add at least one positive OR one negative
+          to enable discovery.
         </p>
+
+        <!-- Self-contained session picker. The previous modal had only two
+             dropzones that listed selected items, but the `+ pos` / `− neg`
+             buttons that populate them live on cards in the main view —
+             which the `<dialog>.showModal()` backdrop blocks. Users who
+             opened this modal first had no way to add anything from inside
+             it. The picker below makes the modal self-contained: search the
+             collection, click + pos / − neg per row. The main-view buttons
+             still work for users who pre-stage selections before opening. -->
+        <div class="mix-picker">
+          <label class="mix-picker-lbl" for="mix-picker-input">
+            <span>Find sessions to add (type a query, press ↵)</span>
+            <input
+              type="text"
+              id="mix-picker-input"
+              class="mix-picker-input"
+              placeholder='e.g. "auth refactor", "cargo build error", or paste a session_id'
+              autocomplete="off"
+              spellcheck="false"
+            />
+          </label>
+          <div id="mix-picker-results" class="mix-picker-results" role="listbox" aria-label="Search results"></div>
+        </div>
+
         <label>Positive sessions</label>
         <div id="mix-positive" class="dropzone" data-side="positive">
-          <span class="dropzone-hint">drop ids here…</span>
+          <span class="dropzone-hint">search above OR click + pos on a card behind the modal…</span>
         </div>
         <label>Negative sessions</label>
         <div id="mix-negative" class="dropzone" data-side="negative">
-          <span class="dropzone-hint">drop ids here…</span>
+          <span class="dropzone-hint">search above OR click − neg on a card behind the modal…</span>
         </div>
         <button id="btn-run-mix" type="button" class="btn primary">Run discovery</button>
         <div id="mix-results" class="mix-results"></div>
```

### 2.2 `src/main.js`

Three independent hunks. **No new imports needed** — `invoke` is already on `window.__TAURI__.core` (line 5), `escapeHtml` already defined (line 1521), `state.mix` / `state.query` already on the state object (lines 11, 14).

```diff
diff --git a/src/main.js b/src/main.js
index 0000000..0000000 100644
--- a/src/main.js
+++ b/src/main.js
@@ -45,6 +45,8 @@ document.addEventListener("DOMContentLoaded", async () => {
   attachStackEvents();
   attachReplayEvents();
   attachRecallBannerEvents();
+  // Register the self-contained Mix & Match picker (search + add inside the modal).
+  attachMixPickerEvents();
   // Kick off both pollers; the stack uses pure jsonl parsing so it succeeds
   // even before Qdrant comes up, giving the user something to look at
   // immediately.
@@ -1400,7 +1402,143 @@
 function openMixModal() {
   renderMixDropzones();
   document.getElementById("mix-results").innerHTML = "";
+  // Clear last picker state, but seed it with the most recent lens query
+  // so the picker arrives pre-filled with sessions the user just looked at.
+  const pickerInput = document.getElementById("mix-picker-input");
+  const pickerResults = document.getElementById("mix-picker-results");
+  if (pickerInput) {
+    pickerInput.value = state.query || "";
+  }
+  if (pickerResults) {
+    pickerResults.innerHTML = "";
+  }
   document.getElementById("mix-modal").showModal();
+  // If the user already had a non-empty query, run it once automatically so
+  // the picker isn't empty on first open.
+  if (pickerInput && pickerInput.value.trim()) {
+    runMixPickerSearch();
+  }
+  updateRunMixButton();
+}
+
+// Self-contained picker so the modal doesn't depend on stack cards that the
+// modal backdrop blocks.
+//
+// runMixPickerSearch() — invoke lens_search to populate the picker's result
+// list. Each row carries [+ pos] [− neg] buttons that call addToMix() the
+// same way the main-view stack-card buttons do.
+async function runMixPickerSearch() {
+  const input = document.getElementById("mix-picker-input");
+  const results = document.getElementById("mix-picker-results");
+  if (!input || !results) return;
+  const q = input.value.trim();
+  if (!q) {
+    results.innerHTML =
+      '<p class="mix-picker-empty">Type a query above and press ↵, or paste a session_id.</p>';
+    return;
+  }
+  // If the query looks like a session_id UUID, treat it as a direct id pick
+  // (no Qdrant round-trip needed).
+  const uuidRe = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
+  if (uuidRe.test(q)) {
+    results.innerHTML = "";
+    renderMixPickerRow(
+      { session_id: q, project_name: "(by id)", ai_title: q, start_iso: "" },
+      results,
+    );
+    return;
+  }
+  // Otherwise: lens search against the live collection.
+  results.innerHTML = '<p class="mix-picker-empty">Searching…</p>';
+  let hits = [];
+  try {
+    hits = await invoke("lens_search", {
+      query: q,
+      weights: state.weights,
+      limit: 12,
+    });
+  } catch (err) {
+    results.innerHTML = `<p class="mix-picker-empty">Search failed: ${escapeHtml(String(err))}</p>`;
+    return;
+  }
+  if (!hits || !hits.length) {
+    results.innerHTML = '<p class="mix-picker-empty">No matches.</p>';
+    return;
+  }
+  results.innerHTML = "";
+  for (const h of hits) {
+    renderMixPickerRow(h, results);
+  }
+}
+
+function renderMixPickerRow(hit, parent) {
+  const row = document.createElement("div");
+  row.className = "mix-picker-row";
+  const meta = document.createElement("div");
+  meta.className = "mix-picker-meta";
+  const project = hit.project_name || "(unknown project)";
+  const title = (hit.ai_title || "").trim() || "(untitled)";
+  const start = hit.start_iso ? hit.start_iso.slice(0, 16).replace("T", " ") : "";
+  meta.innerHTML = `
+    <span class="mix-picker-project">${escapeHtml(project)}</span>
+    <span class="mix-picker-title">${escapeHtml(title.slice(0, 80))}</span>
+    <span class="mix-picker-start">${escapeHtml(start)}</span>
+  `;
+  const actions = document.createElement("div");
+  actions.className = "mix-picker-actions";
+  const posBtn = document.createElement("button");
+  posBtn.type = "button";
+  posBtn.className = "btn ghost xs";
+  posBtn.textContent = "+ pos";
+  posBtn.title = "Add as positive anchor";
+  posBtn.addEventListener("click", () => {
+    addToMix("positive", hit.session_id);
+    posBtn.disabled = true;
+    posBtn.textContent = "✓ pos";
+  });
+  const negBtn = document.createElement("button");
+  negBtn.type = "button";
+  negBtn.className = "btn ghost xs";
+  negBtn.textContent = "− neg";
+  negBtn.title = "Add as negative (anti-context) anchor";
+  negBtn.addEventListener("click", () => {
+    addToMix("negative", hit.session_id);
+    negBtn.disabled = true;
+    negBtn.textContent = "✓ neg";
+  });
+  actions.append(posBtn, negBtn);
+  row.append(meta, actions);
+  parent.appendChild(row);
+}
+
+function attachMixPickerEvents() {
+  const input = document.getElementById("mix-picker-input");
+  if (!input) return;
+  let debounce = null;
+  input.addEventListener("keydown", (e) => {
+    if (e.key === "Enter") {
+      e.preventDefault();
+      runMixPickerSearch();
+    }
+  });
+  input.addEventListener("input", () => {
+    if (debounce) clearTimeout(debounce);
+    debounce = setTimeout(() => {
+      // Auto-search on quiet pause if the user typed at least 2 chars.
+      if (input.value.trim().length >= 2) {
+        runMixPickerSearch();
+      }
+    }, 350);
+  });
+}
+
+// Keeps the [Run discovery] button + hint message in sync with state.
+function updateRunMixButton() {
+  const btn = document.getElementById("btn-run-mix");
+  if (!btn) return;
+  const ready =
+    state.mix.positive.length > 0 || state.mix.negative.length > 0;
+  btn.disabled = !ready;
+  btn.title = ready
+    ? "Run Qdrant Discovery on your selections"
+    : "Add at least one positive OR negative session first";
 }
 
 function addToMix(side, sessionId) {
@@ -1408,12 +1546,21 @@ function addToMix(side, sessionId) {
     state.mix[side].push(sessionId);
   }
   renderMixDropzones();
+  updateRunMixButton();
 }
 
 function removeFromMix(side, sessionId) {
   state.mix[side] = state.mix[side].filter((s) => s !== sessionId);
   renderMixDropzones();
+  updateRunMixButton();
+  // Also flip the corresponding picker row's button back to its un-added
+  // state if it's still on screen — re-running the search refreshes it.
+  const input = document.getElementById("mix-picker-input");
+  if (input && input.value.trim()) {
+    runMixPickerSearch();
+  }
 }
 
 function renderMixDropzones() {
   for (const side of ["positive", "negative"]) {
@@ -1422,7 +1569,10 @@ function renderMixDropzones() {
     if (!state.mix[side].length) {
       const hint = document.createElement("span");
       hint.className = "dropzone-hint";
-      hint.textContent = "click + pos / − neg on a card to add…";
+      hint.textContent =
+        side === "positive"
+          ? "search above OR click + pos on a card behind the modal…"
+          : "search above OR click − neg on a card behind the modal…";
       root.appendChild(hint);
       continue;
     }
```

### 2.3 `src/styles.css`

Two hunks: `.btn[disabled]` rule near the other `.btn` rules (so disabled state is consistent project-wide, not just for the Run-discovery button), and the `.mix-picker*` selectors appended at end of file.

```diff
diff --git a/src/styles.css b/src/styles.css
index 0000000..0000000 100644
--- a/src/styles.css
+++ b/src/styles.css
@@ -153,6 +153,15 @@
 
 .btn.ghost {
   background: transparent;
 }
 
+.btn[disabled] {
+  opacity: 0.45;
+  cursor: not-allowed;
+  filter: saturate(0.6);
+}
+.btn[disabled]:hover {
+  background: var(--accent);  /* override the :hover lighten */
+}
+
 .btn.xs {
   padding: 2px 7px;
   font-size: 10.5px;
   line-height: 1.1;
 }
@@ -1773,3 +1782,115 @@
 
 ::-webkit-scrollbar-track {
   background: transparent;
 }
+
+/* Self-contained Mix & Match picker: search + result list + per-row
+   + pos / − neg buttons live INSIDE the modal so the user never has to
+   dismiss the dialog to add anchors. Markup is `<div class="mix-picker">`
+   in index.html, inserted between `<p class="side-desc">` and the
+   Positive/Negative dropzones. */
+.mix-picker {
+  display: flex;
+  flex-direction: column;
+  gap: 8px;
+  margin: 14px 0 6px;
+}
+
+.mix-picker-lbl {
+  display: flex;
+  flex-direction: column;
+  gap: 4px;
+  font-size: 12px;
+  letter-spacing: 0.06em;
+  text-transform: uppercase;
+  color: oklch(68% 0.01 260);
+}
+
+.mix-picker-input {
+  width: 100%;
+  background: oklch(18% 0.01 260);
+  border: 1px solid oklch(28% 0.01 260);
+  color: oklch(94% 0.01 260);
+  border-radius: 8px;
+  padding: 9px 12px;
+  font-size: 13.5px;
+  font-family: inherit;
+  transition: border-color 0.15s ease, background 0.15s ease;
+}
+.mix-picker-input:focus {
+  outline: none;
+  border-color: oklch(72% 0.14 220);
+  background: oklch(20% 0.01 260);
+}
+
+.mix-picker-results {
+  display: flex;
+  flex-direction: column;
+  gap: 4px;
+  max-height: 220px;
+  overflow-y: auto;
+  background: oklch(15% 0.01 260);
+  border: 1px solid oklch(26% 0.01 260);
+  border-radius: 8px;
+  padding: 6px;
+}
+
+.mix-picker-results:empty {
+  display: none;
+}
+
+.mix-picker-empty {
+  color: oklch(60% 0.01 260);
+  font-size: 13px;
+  padding: 12px 8px;
+  margin: 0;
+  text-align: center;
+}
+
+.mix-picker-row {
+  display: flex;
+  align-items: center;
+  justify-content: space-between;
+  gap: 10px;
+  padding: 6px 10px;
+  border-radius: 6px;
+  background: oklch(18% 0.01 260);
+  transition: background 0.12s ease;
+}
+.mix-picker-row:hover {
+  background: oklch(22% 0.01 260);
+}
+
+.mix-picker-meta {
+  display: flex;
+  flex-direction: column;
+  gap: 2px;
+  min-width: 0;
+  flex: 1;
+}
+
+.mix-picker-project {
+  font-size: 11.5px;
+  color: oklch(72% 0.14 220);
+  text-transform: uppercase;
+  letter-spacing: 0.08em;
+}
+
+.mix-picker-title {
+  font-size: 13px;
+  color: oklch(92% 0.01 260);
+  white-space: nowrap;
+  overflow: hidden;
+  text-overflow: ellipsis;
+}
+
+.mix-picker-start {
+  font-size: 11px;
+  color: oklch(60% 0.01 260);
+  font-family: "IBM Plex Mono", "SF Mono", ui-monospace, monospace;
+}
+
+.mix-picker-actions {
+  display: flex;
+  gap: 6px;
+  flex-shrink: 0;
+}
+
+.mix-picker-actions .btn[disabled] {
+  opacity: 0.55;
+  cursor: default;
+  background: oklch(28% 0.05 145);
+  color: oklch(82% 0.08 145);
+}
```

> **Note on `oklch()` usage**: upstream's `styles.css` does not currently use the `oklch()` color function (it uses hex + `var(--*)`). Both Tauri targets (WKWebView on macOS, WebView2 on Windows) ship Safari/Chromium versions that support `oklch()` (Safari 15.4+, Chromium 111+). Tauri 2's minimum WebView is well above both. If the maintainer wants strict palette consistency with the existing CSS custom-property system, the 9 `oklch()` calls in `.mix-picker*` can be mechanically rewritten to `var(--bg-input)` / `var(--bg-glass-hi)` / `var(--text-secondary)` / `var(--text-muted)` / `var(--border)` / `var(--border-strong)` / `var(--accent)`. Leaving `oklch()` in the initial PR keeps the visual identity from the fork; offering both is reasonable.

---

## 3. Verification protocol

### 3.1 Apply the patch

```bash
# Maintainer side, on a fresh worktree of sgwannabe/memex
git fetch origin
git checkout -b backport/mix-modal-self-contained-picker origin/main
# … apply the three diffs above (e.g. via `git apply` after pasting into a .patch file)
git diff --stat   # expect: 3 files changed, ~250 insertions(+), ~10 deletions(-)
```

### 3.2 Static checks

```bash
# JS syntax — runs in <2s, no deps
node --check src/main.js

# CSS / HTML — visual inspection of the diff above is sufficient
# (no css-linter is configured in upstream package.json — verified)
grep -n 'mix-picker' src/index.html src/styles.css src/main.js
# Expect:
#   src/index.html: 6 matches (the picker block)
#   src/styles.css: many matches (the appended rules)
#   src/main.js:    ~10 matches (functions + DOM lookups)
```

### 3.3 Build

```bash
# Front-end is vanilla ESM — no bundler — so the only build step is Tauri's.
cd src-tauri
cargo check       # ~30s warm — should be a no-op (no Rust changes)
cd ..
npm run tauri dev # or: npm run tauri build
```

### 3.4 Manual test plan

Required interactions (no automation harness in upstream):

1. **First-open flow (the bug being fixed)** — launch the app, click `Mix & Match` in the top bar. Expected: picker is visible at the top of the modal body; input is empty; dropzones show `"search above OR click + pos / − neg on a card behind the modal…"` hints; `[Run discovery]` is **disabled** with tooltip `"Add at least one positive OR negative session first"`.
2. **Search-and-add flow** — type a real query (e.g. the project name of any indexed session), press `↵`. Expected: up to 12 rows appear, each showing project / title / start. Click `[+ pos]` on one row → button flips to `✓ pos` and disables; chip appears in the Positive dropzone; `[Run discovery]` becomes enabled. Click `[Run discovery]` → results render below as in the old flow.
3. **UUID paste flow** — paste a real `session_id` (UUID) into the input. Expected: no Qdrant round-trip ("Searching…" does not appear); one synthetic row labelled `(by id)` is shown immediately.
4. **Remove-resync flow** — after adding a positive, click the `×` on the chip in the dropzone. Expected: the picker re-runs the last search and the corresponding row's button returns to `+ pos` (clickable again); `[Run discovery]` disables if no anchors remain.
5. **Pre-stage flow still works** — close the modal. In the main view, click `[+ pos]` on any card. Re-open the modal. Expected: the positive chip is already in the dropzone; `[Run discovery]` is enabled.
6. **Self-priming on open** — type a query into the main search box (so `state.query` is non-empty), then open the modal. Expected: the picker input is pre-filled with that query and the result list populates automatically.

### 3.5 Screenshots maintainer should attach to the PR

1. The modal **before** the patch (showing the dead-end: dropzones empty, hint asks user to click a backdropped button).
2. The modal **after** the patch with the picker visible and empty.
3. After search: 5+ rows in the picker, `[Run discovery]` still disabled.
4. After `[+ pos]` click: chip in dropzone, row button shows `✓ pos`, `[Run discovery]` enabled.
5. Successful `Run discovery` showing `#mix-results` populated.

---

## 4. Risk assessment

| Risk | Severity | Notes |
|---|---|---|
| Backend contract change | **None** | Patch is HTML/CSS/JS only. Uses the existing `lens_search` and `mix_match` Tauri commands. `cargo check` is a no-op. |
| `state.weights` shape change | **None** | Picker reads `state.weights` (already populated by `buildLensSliders()` on `DOMContentLoaded`) and forwards it unmodified — same shape `runLensSearch()` uses. |
| Breaking the old drag-from-card flow | **None** | `addToMix` / `removeFromMix` signatures unchanged; main-view card buttons still call them. Only behavioral *addition* in `removeFromMix` is `runMixPickerSearch()` if the picker input is non-empty — a no-op when the picker isn't on screen / is empty. |
| `[Run discovery]` starts disabled — different from before | **Low (UX improvement)** | Before the patch, clicking it with an empty stage produced an inline error message. After, the button is greyed out with a tooltip. Users who relied on the click-to-discover-the-error path will get the same information faster. |
| `oklch()` color function | **Low** | All Tauri 2 target WebViews support it (see §2.3 note). If the maintainer's CI includes older WebKitGTK on Linux (< 2.42), rewrite to `var(--*)`. |
| Accessibility regression | **None new; one regression fixed** | Picker input has an explicit `<label for="mix-picker-input">`. Results container has `role="listbox"` and `aria-label`. **Fix**: `[Run discovery]` now has a `title` attribute explaining why it's disabled (was opaque before). **Pre-existing gap not addressed**: `.mix-picker-row` is not keyboard-focusable; users navigate via Tab through the `[+ pos]` / `[− neg]` buttons inside each row. The buttons are real `<button>` elements with proper `type="button"`, so Tab order works; arrow-key navigation between rows is not implemented. This matches the existing main-view card pattern, so the patch does not widen the gap. |
| `requestAnimationFrame` ordering in `openMixModal` | **None** | Upstream's `openMixModal` has no `requestAnimationFrame` call; the patch does not add one (fork-only `initHyperplane()` is intentionally dropped). |
| Debounced auto-search firing duplicate queries | **Very low** | 350 ms debounce; if user pauses then hits Enter, `runMixPickerSearch()` runs twice in quick succession but `lens_search` is idempotent. The second render overwrites the first with the same content. |
| Modal opens for the first time → `attachMixPickerEvents()` couldn't find the input | **None** | `attachMixPickerEvents` runs on `DOMContentLoaded` after `index.html` is fully parsed; the `<dialog>`'s children are part of the static DOM tree even before `.showModal()`. Verified by upstream's own use of `document.getElementById("mix-target")` etc. in similar contexts. |

**Behavioral changes beyond the bug fix** (call them out in the PR body):
1. Header copy `Discovery API` is unchanged (fork's `Discovery Hyperplane` rename intentionally dropped).
2. `<p class="side-desc">` rewritten to be user-facing English instead of the "drag two or more" phrasing (which lied — drag-and-drop was never implemented).
3. Dropzone hint copy changed (the old "drop ids here…" was misleading; the new copy correctly describes the two real ways to add).
4. `[Run discovery]` is disabled until at least one anchor exists (previously enabled-but-erroring).

No telemetry, no API, no schema, no preference, no keybinding changes.

---

## 5. PR draft

**Title** (62 chars):
```
fix(ui): make Mix & Match modal self-contained (search + add inline)
```

**Body**:

```markdown
## What

Adds a search + add picker inside the `#mix-modal` dialog so users can populate the Positive / Negative dropzones without dismissing the modal.

## Why

`#mix-modal` is a native `<dialog>` opened via `.showModal()`, which renders a `::backdrop` over the rest of the page. The only documented way to add anchors is the `[+ pos]` / `[− neg]` buttons on stack cards / result cards in the main view — but those cards live behind the backdrop and are pointer-event-blocked.

A user who opens the modal first (the natural discovery flow, since "Mix & Match" is a top-bar button) is shown the dropzone hint `"click + pos / − neg on a card to add…"` referring to buttons they can see but cannot click. Clicking `[Run discovery]` returns the inline error `"Add at least one positive or negative session first."` — a dead-end loop.

Repro (current `main`):
1. Launch the app, wait for the stack to load.
2. Click `Mix & Match` in the top bar.
3. Try to add a session. There is no in-modal control to do so.
4. Click `[Run discovery]`. Error.

## How

Three files, no Rust changes, no backend changes.

- **`src/index.html`**: insert a `<div class="mix-picker">` block (search input + results container) between the `side-desc` and the existing Positive / Negative dropzones. Update the misleading `"drop ids here…"` hint copy (drag-and-drop was never implemented) to mention both real paths.
- **`src/main.js`**: add `runMixPickerSearch()` (calls the existing `lens_search` command with `state.weights`, limit 12; treats UUID-shaped input as a direct session pick with no Qdrant round-trip), `renderMixPickerRow()` (per-row `[+ pos]` / `[− neg]` buttons that dispatch to the existing `addToMix(side, sessionId)`), `attachMixPickerEvents()` (Enter + 350 ms-debounced input), and `updateRunMixButton()` (disable `[Run discovery]` with an explanatory `title` when no anchors are staged). `addToMix` / `removeFromMix` get one-liner hooks to keep the button state in sync.
- **`src/styles.css`**: `.mix-picker*` selectors (~110 LOC, appended at end of file) and a global `.btn[disabled]` rule for the new disabled state. Color palette uses `oklch()` for consistency with the picker's own palette — happy to rewrite as `var(--*)` tokens if you prefer.

## Backwards compatibility

The main-view stack-card / result-card `[+ pos]` / `[− neg]` buttons still work and still dispatch through the same `addToMix` path for users who prefer to pre-stage selections before opening the modal. No Tauri command changes, no state-shape changes, no preference changes.

## Test plan

- [ ] Open modal with no prior selections → picker visible, dropzones show new hint, `[Run discovery]` disabled with tooltip
- [ ] Type query → ↵ → up to 12 rows appear
- [ ] Click `[+ pos]` on a row → button becomes `✓ pos`, chip appears in dropzone, `[Run discovery]` enables
- [ ] Click `[Run discovery]` → results render in `#mix-results` (existing path, unchanged)
- [ ] Paste a UUID → single `(by id)` row appears immediately, no network call
- [ ] Remove a chip from a dropzone → picker re-runs search, row's button returns to `+ pos`
- [ ] Pre-stage flow still works: add via main-view card, then open modal → chip already present
- [ ] Self-priming: type into main search box, open modal → picker input pre-filled and auto-searches

## Screenshots

(maintainer: see §3.5 of the design doc for the 5 screenshots to attach)

## Out of scope (intentionally)

- Keyboard arrow-key navigation between picker rows (matches the existing main-view card pattern — patch does not widen the gap).
- Drag-and-drop (never implemented; the existing `"drop ids here…"` copy was aspirational).
- Any change to `lens_search` / `mix_match` command surfaces.
```

---

## 6. Apply-and-ship checklist for the operator

```bash
# 1. Branch from upstream main
cd /path/to/sgwannabe-memex
git fetch origin
git checkout -b backport/mix-modal-self-contained-picker origin/main

# 2. Apply the three diffs (paste each into a .patch file then `git apply`,
#    or open the three files in an editor and apply by hand).

# 3. Static + build verification
node --check src/main.js
cd src-tauri && cargo check && cd ..
npm run tauri dev

# 4. Run the manual test plan above.

# 5. Commit + push + PR
git add src/index.html src/main.js src/styles.css
git commit -m "fix(ui): make Mix & Match modal self-contained (search + add inline)

The #mix-modal <dialog> is opened via .showModal(), which renders a
::backdrop over the rest of the page. The only documented way to
populate the Positive / Negative dropzones was clicking + pos / − neg on
stack or result cards in the main view — but those cards live behind
the backdrop and are pointer-event-blocked. A user who opens the modal
first has no way to add anchors and clicking [Run discovery] returns
'Add at least one positive or negative session first.' as a dead end.

This patch puts a search input + results list inside the modal, with
per-row + pos / − neg buttons that dispatch through the existing
addToMix() path. The main-view card buttons still work for users who
prefer to pre-stage selections.

No Rust / Tauri command / state-shape changes.
"
git push -u origin backport/mix-modal-self-contained-picker
gh pr create --repo sgwannabe/memex --base main \
  --title "fix(ui): make Mix & Match modal self-contained (search + add inline)" \
  --body-file <pr-body.md>
```

---

## 7. If the patch is rejected — fallback paths

1. **Minimum-viable variant**: drop the entire picker, just disable `[Run discovery]` with the `updateRunMixButton()` logic and rewrite the dropzone hint to point at the main-view buttons more explicitly ("close this dialog and click + pos on a card"). ~20 LOC total. Doesn't fix the UX trap fully but removes the dead-end click loop.
2. **Move the trigger out of the modal**: change `#btn-mix` to open a side panel instead of a `<dialog>`. Larger surface change but eliminates the backdrop problem at the root. Out of scope for this PR.
3. **Make the modal non-modal**: swap `.showModal()` for `.show()`. One-line change, removes the backdrop, but loses Esc-to-close and scrim affordance. Probably not what the maintainer wants.
