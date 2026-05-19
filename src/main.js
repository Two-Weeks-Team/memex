// Memex frontend — vanilla JS shell wiring 5 Qdrant-backed commands.
// Tauri's `withGlobalTauri: true` puts the IPC bridge on window.__TAURI__,
// so we can stay plain ESM without a build step.

const { invoke } = window.__TAURI__.core;

// Six dense lens vectors: original 5 + content_late (multivector, P2 KA-01).
const LENSES = ["content", "tool", "path", "error", "code", "content_late"];
// Lens defaults: content_late starts at 0 (off) to preserve legacy behavior.
const LENS_DEFAULT = {
  content: 1.0,
  tool: 1.0,
  path: 1.0,
  error: 1.0,
  code: 1.0,
  content_late: 0.0,
};

// WOW-3: vectors we render as bar segments. Includes sparse counterparts that
// `lens_search_v2` populates in `score_breakdown.per_vector`.
const BAR_VECTORS = [
  "content",
  "tool",
  "path",
  "error",
  "code",
  "content_late",
  "path_sparse",
  "tool_sparse",
];

// WOW: motion / a11y kill switch — disable trail SVG, hyperplane rotation,
// and bar transitions when the user prefers reduced motion.
const REDUCED_MOTION = window.matchMedia
  ? window.matchMedia("(prefers-reduced-motion: reduce)")
  : { matches: false };

const state = {
  query: "",
  weights: { ...LENS_DEFAULT },
  // WOW-3: fusion mode ("formula" | "rrf") and optional MMR diversity (0..1)
  // forwarded to lens_search_v2.
  fusion: "formula",
  diversity: null,
  hits: [],
  selected: null,
  // WOW-5: mix state evolves from {pos, neg} arrays to a richer model that
  // also carries pair context for `mix_match_with_pairs` (P4 KB-03).
  mix: { positive: [], negative: [], pairs: [], target: null, lastQuery: null },
  collectionPoints: 0,
  // B3: monotonically-increasing query id; renderResults drops responses
  // whose generation is older than the latest dispatched query.
  queryGen: 0,
  // Time Machine stack: loaded on boot via list_sessions (no Qdrant needed),
  // shown when there's no active search query.
  stack: [],
  stackFocus: 0,
  mode: "stack", // "stack" | "search"
  // WOW-1: heat-trail neighbor cache keyed by session_id so repeated hovers
  // over the same card don't re-issue a lens_search_v2 round trip.
  heatTrail: { cache: new Map(), hoveredId: null, raf: null },
  // WOW-2: source_agent lookup so the topology agent filter can toggle
  // sessions in/out without backend roundtrips.
  agentByNode: new Map(),
  agentFilter: "both", // "claude_code" | "codex" | "both"
  // WOW-4: predictive neighbor grid cache for the Predict cinematic panel.
  predictGrid: { sessionId: null, items: [] },
  // WOW-5: hyperplane render loop handle so we can cancel it on close.
  hyperplane: { raf: null, lastFrameMs: 0 },
  replay: {
    sessionId: null,
    turns: [],
    cursor: 0,
    playing: false,
    speedMs: 500,
    timer: null,
  },
  recall: {
    dismissedKeys: new Set(),
    lastBannerError: null,
    // P2: short-lived cache so repeated polls of the same error_text don't
    // re-embed + re-search every 12 s.
    cache: new Map(), // error_text → { hits, ts }
  },
};

const RECALL_CACHE_TTL_MS = 60_000;
const RECALL_CACHE_MAX = 50;

document.addEventListener("DOMContentLoaded", async () => {
  buildLensSliders();
  attachEvents();
  attachStackEvents();
  attachReplayEvents();
  attachRecallBannerEvents();
  // WOW: register controls + hover handlers for the 5 visual integrations.
  attachWow3Controls();
  attachHeatTrailHandlers();
  attachAgentFilterHandlers();
  // WOW-5: register the self-contained Mix & Match picker (search + add).
  attachMixPickerEvents();
  // P8 — `memex://<route>` deep links: register listener + handle launch URL.
  attachDeepLinkRoutes();
  // Kick off both pollers; the stack uses pure jsonl parsing so it succeeds
  // even before Qdrant comes up, giving the user something to look at
  // immediately.
  loadInitialStack();
  await pollUntilReady();
  startRecallPolling();
});

// ---------------------------------------------------------------------------
// P8 — Deep-link router
// ---------------------------------------------------------------------------
// Maps `memex://<route>` to one of the 5 WOW surfaces, then focuses the
// matching control. The plugin emits a `deep-link://new-url` event back to
// the webview; we also poll `getCurrent()` once to catch the URL the app
// was launched with (cold-start case).
//
// Routes:
//   memex://timemachine — return to main Time Machine view (clear modals)
//   memex://topology    — open Topology galaxy modal
//   memex://lens        — focus the search input (lens is the search box)
//   memex://predict     — focus the predict panel for the active session
//   memex://mix-match   — open the Mix & Match modal
function dispatchDeepLink(rawUrl) {
  if (!rawUrl) return;
  let route;
  try {
    const u = new URL(rawUrl);
    if (u.protocol !== "memex:") return;
    // ROBUSTNESS FIX (Gemini PR #10 review, main.js:133):
    //   - Prefer `hostname` over `host` so a stray port like
    //     `memex://topology:8080` doesn't end up as `topology:8080`.
    //   - Trim leading AND trailing slashes from pathname fallback so
    //     `memex:topology/` is treated the same as `memex:topology`.
    // memex://topology      → hostname="topology", pathname=""
    // memex://mix-match     → hostname="mix-match", pathname=""
    // memex:topology        → hostname="", pathname="topology" (handled here)
    route = (u.hostname || u.pathname.replace(/^\/+|\/+$/g, ""))
      .trim()
      .toLowerCase();
  } catch {
    return;
  }
  // Close any open modal first; the target route opens its own if needed.
  document.querySelectorAll("dialog[open]").forEach((d) => d.close());
  switch (route) {
    case "":
    case "timemachine":
    case "time-machine":
    case "stack":
      // Already on main view after closing modals; focus the search input
      // so the user has a sensible keyboard target.
      document.getElementById("search-input")?.focus();
      break;
    case "topology":
    case "galaxy":
      document.getElementById("btn-topology")?.click();
      break;
    case "lens":
    case "search":
      document.getElementById("search-input")?.focus();
      document.getElementById("search-input")?.select?.();
      break;
    case "predict":
    case "prediction":
      // IDEMPOTENCY FIX (Gemini PR #10 review, main.js:167): Predict needs an
      // active session, but the previous implementation ALWAYS clicked the
      // first card — which forcibly stole the user's existing selection
      // every time `memex://predict` was triggered (e.g., from a recurring
      // notification deep-link). Respect `state.selected` if already set;
      // only auto-select the topmost card when there is no active session.
      if (state.selected) {
        // Scroll the existing selection into view so the user knows where
        // the predict surface is anchored.
        const existing = document.querySelector(
          `#results [data-session-id="${state.selected}"]`
        );
        existing?.scrollIntoView?.({ behavior: "smooth", block: "center" });
      } else {
        const firstCard = document.querySelector(
          "#results .stack-card, #results .card"
        );
        firstCard?.scrollIntoView?.({ behavior: "smooth", block: "center" });
        firstCard?.click?.();
      }
      break;
    case "mix":
    case "mix-match":
    case "mixmatch":
    case "discovery":
      document.getElementById("btn-mix")?.click();
      break;
    default:
      // Unknown route — fall back to focusing the search input.
      document.getElementById("search-input")?.focus();
      break;
  }
}

async function attachDeepLinkRoutes() {
  // Guard: feature only available inside the Tauri runtime.
  if (typeof window.__TAURI__ === "undefined") return;
  const events = window.__TAURI__.event;
  if (events && typeof events.listen === "function") {
    try {
      await events.listen("deep-link://new-url", (event) => {
        // Plugin payload is an array of URLs (Tauri v2 multi-URL launch).
        const urls = Array.isArray(event?.payload)
          ? event.payload
          : [event?.payload].filter(Boolean);
        for (const u of urls) dispatchDeepLink(u);
      });
    } catch {
      // Listener registration is best-effort; skip in non-Tauri builds.
    }
  }
  // Cold-start: if the app was launched via `open memex://…`, the plugin
  // exposes the launch URL via the `plugin:deep-link|get_current` IPC.
  try {
    const launch = await window.__TAURI__.core?.invoke?.(
      "plugin:deep-link|get_current"
    );
    if (Array.isArray(launch)) {
      for (const u of launch) dispatchDeepLink(u);
    } else if (typeof launch === "string") {
      dispatchDeepLink(launch);
    }
  } catch {
    // Plugin not available (browser preview, missing capability) — ignore.
  }
}

// ---------------------------------------------------------------------------
// Time Machine stack — initial view + arrow/wheel nav
// ---------------------------------------------------------------------------

async function loadInitialStack() {
  try {
    const sessions = await invoke("list_sessions", { limit: 60 });
    state.stack = sessions || [];
    state.stackFocus = 0;
    if (state.mode === "stack") renderStack();
  } catch (err) {
    console.warn("list_sessions failed:", err);
  }
}

function attachStackEvents() {
  document.addEventListener("keydown", (e) => {
    if (state.mode !== "stack") return;
    if (e.target.tagName === "INPUT" || e.target.tagName === "TEXTAREA") return;
    if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
      e.preventDefault();
      advanceStack(+1); // older
    } else if (e.key === "ArrowDown" || e.key === "ArrowRight") {
      e.preventDefault();
      advanceStack(-1); // newer
    } else if (e.key === "Enter") {
      const focused = state.stack[state.stackFocus];
      if (focused) selectSession(focused.session_id);
    }
  });
  const results = document.getElementById("results");
  let wheelAccum = 0;
  results.addEventListener(
    "wheel",
    (e) => {
      if (state.mode !== "stack") return;
      e.preventDefault();
      wheelAccum += e.deltaY;
      while (Math.abs(wheelAccum) > 60) {
        const step = wheelAccum > 0 ? +1 : -1;
        advanceStack(step);
        wheelAccum -= step * 60;
      }
    },
    { passive: false },
  );
}

function advanceStack(direction) {
  const total = state.stack.length;
  if (!total) return;
  const next = Math.max(0, Math.min(total - 1, state.stackFocus + direction));
  if (next === state.stackFocus) return;
  state.stackFocus = next;
  renderStack();
}

function renderStack() {
  const root = document.getElementById("results");
  // Remove search-mode cards + empty hint, leave stack-card nodes for diff.
  root.querySelectorAll(".card, .empty").forEach((n) => n.remove());

  if (!state.stack.length) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "Scanning ~/.claude/projects…";
    root.appendChild(empty);
    return;
  }

  // Re-render the stack window (focus +/- a few layers).
  root.querySelectorAll(".stack-card").forEach((n) => n.remove());

  const window = 6; // visible behind the focused card
  const start = Math.max(0, state.stackFocus - 1);
  const end = Math.min(state.stack.length, state.stackFocus + window + 1);

  for (let i = start; i < end; i++) {
    const s = state.stack[i];
    const layer = i - state.stackFocus;
    const card = document.createElement("article");
    card.className = "stack-card";
    card.dataset.layer = String(layer);
    card.dataset.sessionId = s.session_id;
    const ts = (s.start_iso || "").slice(0, 16).replace("T", " ");
    const title = s.ai_title || "(untitled)";
    const errBadge = s.has_errors ? '<span class="badge err">errors</span>' : "";
    card.innerHTML = `
      <header>
        <span class="proj">${escapeHtml(s.project_name || "?")}</span>
        <span class="ts">${ts}</span>
        ${errBadge}
      </header>
      <h3 class="title">${escapeHtml(title)}</h3>
      <div class="meta-row">
        <span class="meta">${s.user_turns} user · ${s.assistant_turns} assistant · ${s.tool_count} tools</span>
        <span class="branch">${escapeHtml(s.git_branch || "-")}</span>
      </div>
      <footer>
        <code class="sid">${s.session_id.slice(0, 8)}…</code>
        <button class="btn ghost xs" data-action="replay">Replay</button>
        <button class="btn ghost xs" data-action="add-positive">+ pos</button>
        <button class="btn ghost xs" data-action="add-negative">− neg</button>
      </footer>
    `;
    card.addEventListener("click", () => {
      if (layer === 0) {
        selectSession(s.session_id);
      } else {
        advanceStack(layer);
      }
    });
    card
      .querySelectorAll("button[data-action]")
      .forEach((btn) =>
        btn.addEventListener("click", (e) => {
          e.stopPropagation();
          if (btn.dataset.action === "replay") {
            openReplay(s.session_id, s);
          } else {
            const side =
              btn.dataset.action === "add-positive" ? "positive" : "negative";
            addToMix(side, s.session_id);
          }
        }),
      );
    root.appendChild(card);
  }

  // Bottom HUD — chip-shaped, glassy, readable. Position + nav hints.
  const counter = document.createElement("div");
  counter.className = "stack-counter";
  counter.innerHTML = `
    <span class="pos">${state.stackFocus + 1}<span class="total"> / ${state.stack.length}</span></span>
    <span class="sep"></span>
    <span class="hint"><span class="kbd">↑</span><span class="kbd">↓</span> time-travel</span>
    <span class="sep"></span>
    <span class="hint"><span class="kbd">⏎</span> open</span>
  `;
  root.appendChild(counter);

  if (state.stack[state.stackFocus]) {
    // Lazy-prefetch the inspector for the focused card so it feels instant.
    selectSession(state.stack[state.stackFocus].session_id, { silent: true });
  }
}

function enterSearchMode() {
  state.mode = "search";
  // WOW-1: drop any active heat-trail when leaving the stack surface so the
  // SVG doesn't render stale curves over the new search cards.
  clearHeatTrail();
  document
    .getElementById("results")
    .querySelectorAll(".stack-card, .stack-counter")
    .forEach((n) => n.remove());
}

function enterStackMode() {
  state.mode = "stack";
  document
    .getElementById("results")
    .querySelectorAll(".card")
    .forEach((n) => n.remove());
  renderStack();
}

async function pollUntilReady(attempt = 0) {
  // AppState is `.manage()`d eagerly now, but its slots (Qdrant + fastembed)
  // init lazily on the first command call. First-time launch may need ~10 s
  // to download the BGE-small ONNX model (~130 MB), so we poll patiently and
  // separate the "still warming up" UI from the "real problem" UI.
  const MAX_ATTEMPTS = 240; // 240 × 500 ms = 120 s, comfortable for cold start
  const RETRY_MS = 500;
  try {
    const info = await invoke("collection_info");
    state.collectionPoints = info.points_count;
    setStatus(
      `Connected — ${info.points_count} sessions indexed (${info.collection})`,
    );
    document.getElementById("collection-info").textContent =
      `· ${info.points_count} sessions`;
    // If a session was already selected while Qdrant was still warming
    // up (e.g., initial stack auto-select fired before this poll
    // succeeded), kick its prediction now.
    if (state.selected) schedulePrediction(state.selected);
  } catch (err) {
    const msg = String(err);
    // Distinguish: still cold-starting vs. true failure (Qdrant down / model
    // load borked). Keep retrying in both cases but show an actionable hint
    // once we've waited a while.
    if (attempt < MAX_ATTEMPTS) {
      const sec = Math.round((attempt * RETRY_MS) / 1000);
      let hint = `Bootstrapping… (${sec}s)`;
      if (msg.includes("could not connect to Qdrant") || msg.includes("connection")) {
        hint = `Qdrant not reachable on :6334 — start it with \`./.qdrant/qdrant\` then this banner will clear automatically.`;
      } else if (msg.includes("fastembed") || msg.includes("BGE")) {
        hint = `Loading BGE-small embedder (first launch may take ~30s for model download)…`;
      } else if (msg.includes("state not managed")) {
        hint = `Warming up… (${sec}s)`;
      }
      setStatus(hint);
      setTimeout(() => pollUntilReady(attempt + 1), RETRY_MS);
      return;
    }
    setStatus(`Memex couldn't bootstrap after 2 minutes — last error: ${msg}`);
  }
}

function attachEvents() {
  const input = document.getElementById("search-input");
  input.addEventListener("input", debounce(onSearchInput, 200));
  document.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
      e.preventDefault();
      input.focus();
      input.select();
    }
  });

  document.getElementById("btn-topology").addEventListener("click", onTopology);
  document.getElementById("btn-mix").addEventListener("click", openMixModal);
  document.getElementById("btn-snapshot").addEventListener("click", onSnapshot);
  document.getElementById("btn-refresh").addEventListener("click", onRefresh);
  document.getElementById("btn-reset-lens").addEventListener("click", resetLens);
  document.getElementById("btn-recall").addEventListener("click", onRecall);
  document.getElementById("btn-run-mix").addEventListener("click", runMix);

  for (const closer of document.querySelectorAll("[data-close]")) {
    closer.addEventListener("click", (e) => {
      e.target.closest("dialog").close();
    });
  }

  // Tear down the WebGL scene when the topology modal closes so we don't
  // leak Three.js renderers across re-opens.
  document.getElementById("topology-modal").addEventListener("close", () => {
    disposeTopology();
  });
}

async function onSearchInput(e) {
  state.query = e.target.value.trim();
  if (!state.query) {
    // Returning to stack mode — drop any search cards, re-render the stack.
    enterStackMode();
    return;
  }
  enterSearchMode();
  await runLensSearch();
}

async function runLensSearch() {
  const gen = ++state.queryGen;
  const t0 = performance.now();
  setStatus(`Searching "${state.query}"…`);
  // WOW-3: prefer lens_search_v2 for score_breakdown; fall back to legacy
  // lens_search when v2 isn't compiled into the binary (graceful degrade).
  const weights = lensWeightsForV2();
  state.mix.lastQuery = state.query;
  try {
    let hits;
    try {
      hits = await invoke("lens_search_v2", {
        query: state.query,
        weights,
        limit: 20,
      });
      // Normalize v2 → unified shape (carries score_breakdown).
      hits = (hits || []).map(normalizeLensResult);
    } catch (v2Err) {
      // OBSERVABILITY (Gemini PR #7 review, main.js:390): the v2 call may
      // fail for reasons OTHER than "not deployed yet" (Qdrant transient
      // 503, formula validation error after a schema migration, etc.) —
      // log so a developer hitting this fallback can diagnose without
      // having to redeploy a debug build. The fallback path itself is the
      // safe behavior; logging adds no extra work in the happy path.
      console.warn("lens_search_v2 failed, falling back to legacy:", v2Err);
      // Strip content_late from the weights map so the legacy command
      // doesn't trip on an unknown key.
      const legacyWeights = { ...weights };
      delete legacyWeights.content_late;
      delete legacyWeights.diversity;
      delete legacyWeights.fusion;
      const raw = await invoke("lens_search", {
        query: state.query,
        weights: legacyWeights,
        limit: 20,
      });
      hits = (raw || []).map(legacyHitToLensResult);
    }
    if (gen !== state.queryGen) return;
    state.hits = hits;
    renderResults(hits);
    setStatus(`${hits.length} hits for "${state.query}"`);
    setLatency(Math.round(performance.now() - t0));
  } catch (err) {
    if (gen !== state.queryGen) return;
    setStatus(`Search failed: ${err}`);
  }
}

// WOW-3: serialize the lens state into the LensWeights shape that
// `lens_search_v2` expects (Rust `crate::lens::LensWeights`).
function lensWeightsForV2() {
  const w = { ...state.weights };
  w.fusion = state.fusion === "rrf" ? "rrf" : "formula";
  // `diversity: null` → omit so serde default kicks in. Sent as a number
  // (0..1) when set explicitly.
  if (state.diversity != null && Number.isFinite(state.diversity)) {
    w.diversity = state.diversity;
  }
  return w;
}

// WOW-3: massage a lens_search_v2 LensResult into the shape the rest of the
// UI works with. Carries score_breakdown for the contribution bars.
function normalizeLensResult(r) {
  if (!r) return r;
  // Backend already returns the right shape; we just compute a derived
  // `vector_scores` so legacy renderers (vec-breakdown chips) keep working.
  const breakdown = r.score_breakdown || {};
  const per = breakdown.per_vector || {};
  return {
    session_id: r.session_id,
    score: r.score,
    project_name: r.project_name,
    ai_title: r.ai_title,
    start_iso: r.start_iso,
    score_breakdown: breakdown,
    payload_json: r.payload_json || null,
    // Legacy compatibility shim.
    vector_scores: per,
  };
}

// WOW-3: when v2 isn't available, project a legacy SearchHit into the
// shared LensResult shape so the same renderer works without branches.
function legacyHitToLensResult(h) {
  if (!h) return h;
  return {
    session_id: h.session_id,
    score: h.score,
    project_name: h.project_name,
    ai_title: h.ai_title,
    start_iso: h.start_iso,
    score_breakdown: {
      per_vector: h.vector_scores || {},
      recency_factor: 0,
      has_errors_boost: 0,
      final_score: h.score,
    },
    payload_json: null,
    vector_scores: h.vector_scores || {},
  };
}

function buildLensSliders() {
  const root = document.getElementById("lens-sliders");
  root.innerHTML = "";
  for (const name of LENSES) {
    const defaultValue = LENS_DEFAULT[name] ?? 1.0;
    // content_late ranges to 2.0 like the others; the multivector lens is
    // typically run at 0 (off) or low values to avoid drowning the bar.
    const wrap = document.createElement("div");
    wrap.className = "slider";
    wrap.dataset.lens = name;
    wrap.innerHTML = `
      <div class="slider-label">
        <span>${name}</span>
        <span class="slider-value" data-for="${name}">${defaultValue.toFixed(2)}</span>
      </div>
      <input
        type="range"
        min="0"
        max="2"
        step="0.05"
        value="${defaultValue}"
        data-name="${name}"
        aria-label="${name} lens weight"
      />
    `;
    const input = wrap.querySelector("input");
    const value = wrap.querySelector(".slider-value");
    input.addEventListener("input", (e) => {
      const v = parseFloat(e.target.value);
      state.weights[name] = v;
      value.textContent = v.toFixed(2);
    });
    input.addEventListener(
      "change",
      debounce(() => {
        if (state.query) runLensSearch();
      }, 150),
    );
    root.appendChild(wrap);
  }
}

function resetLens() {
  for (const name of LENSES) state.weights[name] = LENS_DEFAULT[name] ?? 1.0;
  document
    .querySelectorAll("#lens-sliders input")
    .forEach((i) => {
      const name = i.dataset.name;
      i.value = String(LENS_DEFAULT[name] ?? 1.0);
    });
  document
    .querySelectorAll("#lens-sliders .slider-value")
    .forEach((s) => {
      const name = s.dataset.for;
      s.textContent = (LENS_DEFAULT[name] ?? 1.0).toFixed(2);
    });
  // WOW-3: also reset fusion + diversity controls.
  state.fusion = "formula";
  state.diversity = null;
  for (const pill of document.querySelectorAll(".fusion-pill")) {
    const isActive = pill.dataset.fusion === "formula";
    pill.classList.toggle("active", isActive);
    pill.setAttribute("aria-checked", isActive ? "true" : "false");
  }
  const ds = document.getElementById("diversity-slider");
  if (ds) ds.value = "0";
  const dv = document.getElementById("diversity-value");
  if (dv) dv.textContent = "off";
  if (state.query) runLensSearch();
}

// WOW-3: wire fusion toggle (Formula / RRF) and the MMR diversity slider.
// A 0 value on the slider is treated as `null` — that's "MMR off" so the
// content prefetch stays a plain NearestQuery.
function attachWow3Controls() {
  for (const pill of document.querySelectorAll(".fusion-pill")) {
    pill.addEventListener("click", () => {
      const mode = pill.dataset.fusion;
      if (state.fusion === mode) return;
      state.fusion = mode;
      for (const p of document.querySelectorAll(".fusion-pill")) {
        const isActive = p === pill;
        p.classList.toggle("active", isActive);
        p.setAttribute("aria-checked", isActive ? "true" : "false");
      }
      if (state.query) runLensSearch();
    });
  }
  const slider = document.getElementById("diversity-slider");
  const value = document.getElementById("diversity-value");
  if (!slider || !value) return;
  slider.addEventListener("input", (e) => {
    const raw = parseFloat(e.target.value);
    if (!raw || raw <= 0.001) {
      state.diversity = null;
      value.textContent = "off";
    } else {
      state.diversity = raw;
      value.textContent = raw.toFixed(2);
    }
  });
  slider.addEventListener(
    "change",
    debounce(() => {
      if (state.query) runLensSearch();
    }, 180),
  );
}

function renderResults(hits) {
  const root = document.getElementById("results");
  root.querySelectorAll(".card").forEach((c) => c.remove());
  document.getElementById("results-empty").style.display = hits.length ? "none" : "";
  for (const h of hits) {
    const card = document.createElement("article");
    card.className = "card";
    card.dataset.sessionId = h.session_id;
    const ts = (h.start_iso || "").slice(0, 16).replace("T", " ");
    const title = h.ai_title || "(untitled)";
    // WOW-3: render the stacked contribution bar from score_breakdown.per_vector
    // if available; otherwise show a flat bar (legacy lens_search fallback).
    const barHtml = renderContributionBar(h.score_breakdown);
    const recencyChip = renderScoreChips(h.score_breakdown);
    card.innerHTML = `
      <header>
        <span class="score">${(h.score ?? 0).toFixed(3)}</span>
        <span class="proj">${escapeHtml(h.project_name || "?")}</span>
        <span class="ts">${ts}</span>
      </header>
      <h3 class="title">${escapeHtml(title)}</h3>
      ${barHtml}
      ${recencyChip}
      <footer>
        <code class="sid">${h.session_id}</code>
        <button class="btn ghost xs" data-action="replay">Replay</button>
        <button class="btn ghost xs" data-action="add-positive">+ pos</button>
        <button class="btn ghost xs" data-action="add-negative">− neg</button>
      </footer>
    `;
    card.addEventListener("click", () => selectSession(h.session_id));
    card
      .querySelectorAll("button[data-action]")
      .forEach((btn) =>
        btn.addEventListener("click", (e) => {
          e.stopPropagation();
          if (btn.dataset.action === "replay") {
            openReplay(h.session_id, h);
          } else {
            const side =
              btn.dataset.action === "add-positive" ? "positive" : "negative";
            addToMix(side, h.session_id);
          }
        }),
      );
    root.appendChild(card);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// WOW-1: Time Machine Heat Trail
// ─────────────────────────────────────────────────────────────────────────────
// On hover of a stack card, fetch up to 5 nearest neighbors and draw SVG
// bezier curves to whichever ones happen to be visible in the stack window.
// Colors are bucketed by similarity score (purple > cyan > yellow).

const HEAT_TOP_K = 5;
const HEAT_FETCH_DELAY_MS = 140;
const HEAT_COLOR_HIGH = "oklch(70% 0.18 290)"; // > 0.7
const HEAT_COLOR_MID = "oklch(75% 0.13 220)"; //  0.5 < s ≤ 0.7
const HEAT_COLOR_LOW = "oklch(80% 0.15 90)"; // ≤ 0.5
let heatHoverTimer = null;

function attachHeatTrailHandlers() {
  const root = document.getElementById("results");
  if (!root) return;
  // Pointer-leave on the section itself clears the trail when the cursor
  // exits the stack region entirely.
  root.addEventListener("mouseleave", clearHeatTrail);
  // Listen on the document for capture-phase hover so we don't have to
  // rebind handlers every renderStack() pass.
  root.addEventListener("mouseover", (e) => {
    const card = e.target.closest(".stack-card");
    if (!card) return;
    const sid = card.dataset.sessionId;
    if (!sid || state.heatTrail.hoveredId === sid) return;
    state.heatTrail.hoveredId = sid;
    if (heatHoverTimer) clearTimeout(heatHoverTimer);
    heatHoverTimer = setTimeout(() => loadHeatNeighbors(sid, card), HEAT_FETCH_DELAY_MS);
  });
  root.addEventListener("mouseout", (e) => {
    const next = e.relatedTarget;
    if (next && next.closest && next.closest(".stack-card")) return;
    // Moving between cards is handled by mouseover; only clear when leaving
    // a card without entering another one in the same tick.
    setTimeout(() => {
      const stillHover = root.querySelector(".stack-card:hover");
      if (!stillHover) clearHeatTrail();
    }, 30);
  });
}

async function loadHeatNeighbors(sessionId, cardEl) {
  if (REDUCED_MOTION.matches) return; // a11y: skip motion entirely.
  if (state.mode !== "stack") return;
  // Cache hit → draw immediately.
  const cached = state.heatTrail.cache.get(sessionId);
  if (cached) {
    drawHeatTrail(cardEl, cached);
    return;
  }
  const seed = state.stack.find((s) => s.session_id === sessionId);
  const query = (seed?.ai_title || seed?.project_name || "").trim();
  if (!query) return;
  try {
    let neighbors;
    try {
      const raw = await invoke("lens_search_v2", {
        query,
        weights: lensWeightsForV2(),
        limit: HEAT_TOP_K + 4,
      });
      neighbors = (raw || []).map(normalizeLensResult);
    } catch {
      const legacy = await invoke("lens_search", {
        query,
        weights: { content: 1, tool: 1, path: 1, error: 1, code: 1 },
        limit: HEAT_TOP_K + 4,
      });
      neighbors = (legacy || []).map(legacyHitToLensResult);
    }
    // Drop self + cap to K.
    const filtered = (neighbors || [])
      .filter((n) => n.session_id !== sessionId)
      .slice(0, HEAT_TOP_K);
    state.heatTrail.cache.set(sessionId, filtered);
    // Only draw if the user is still hovering the same card.
    if (state.heatTrail.hoveredId !== sessionId) return;
    drawHeatTrail(cardEl, filtered);
  } catch (err) {
    // Quiet: hover is a courtesy, failure shouldn't surface to the user.
    console.warn("heat-trail fetch failed:", err);
  }
}

function drawHeatTrail(cardEl, neighbors) {
  const svg = document.getElementById("heat-trail");
  const chip = document.getElementById("heat-chip");
  if (!svg || !cardEl) return;
  const root = document.getElementById("results");
  const rect = root.getBoundingClientRect();
  svg.setAttribute("viewBox", `0 0 ${rect.width} ${rect.height}`);
  svg.innerHTML = "";

  const start = centerInside(cardEl, root);
  let drewAny = 0;
  for (const n of neighbors) {
    const targetCard = root.querySelector(
      `.stack-card[data-session-id="${cssEscape(n.session_id)}"]`,
    );
    if (!targetCard) continue;
    const end = centerInside(targetCard, root);
    const color = heatColor(n.score);
    // Curved bezier — control point pulled toward the vertical midline so
    // trails arc gracefully even when start/end are near-collinear.
    const cx = (start.x + end.x) / 2;
    const cy = Math.min(start.y, end.y) - 60;
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute(
      "d",
      `M ${start.x} ${start.y} Q ${cx} ${cy} ${end.x} ${end.y}`,
    );
    path.setAttribute("stroke", color);
    path.setAttribute("stroke-width", String(1.6 + Math.max(0, n.score) * 2.4));
    path.setAttribute("fill", "none");
    path.setAttribute("opacity", String(0.45 + n.score * 0.45));
    path.setAttribute("stroke-linecap", "round");
    svg.appendChild(path);
    // End-cap dot.
    const dot = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    dot.setAttribute("cx", String(end.x));
    dot.setAttribute("cy", String(end.y));
    dot.setAttribute("r", "5");
    dot.setAttribute("fill", color);
    dot.setAttribute("opacity", "0.85");
    svg.appendChild(dot);
    drewAny++;
  }
  if (!drewAny) {
    svg.innerHTML = "";
  }

  // WOW-1: enrich chip — render up to 4 of 5 enrich fields. Pulled from the
  // focused card's payload so the user can see context without clicking.
  renderHeatChip(cardEl);
}

function heatColor(s) {
  if (typeof s !== "number") return HEAT_COLOR_LOW;
  if (s > 0.7) return HEAT_COLOR_HIGH;
  if (s > 0.5) return HEAT_COLOR_MID;
  return HEAT_COLOR_LOW;
}

function centerInside(el, parent) {
  const er = el.getBoundingClientRect();
  const pr = parent.getBoundingClientRect();
  return {
    x: er.left - pr.left + er.width / 2,
    y: er.top - pr.top + er.height / 2,
  };
}

function cssEscape(s) {
  if (window.CSS && window.CSS.escape) return window.CSS.escape(s);
  return String(s).replace(/["\\]/g, "\\$&");
}

function clearHeatTrail() {
  if (heatHoverTimer) {
    clearTimeout(heatHoverTimer);
    heatHoverTimer = null;
  }
  state.heatTrail.hoveredId = null;
  const svg = document.getElementById("heat-trail");
  if (svg) svg.innerHTML = "";
  const chip = document.getElementById("heat-chip");
  if (chip) chip.classList.add("hidden");
}

async function renderHeatChip(cardEl) {
  const chip = document.getElementById("heat-chip");
  if (!chip) return;
  const sid = cardEl.dataset.sessionId;
  let payload = null;
  try {
    payload = await invoke("get_session", { sessionId: sid });
  } catch {
    /* ignore */
  }
  // RACE FIX (Codex PR #7 review, main.js:826): the IPC round-trip can
  // take long enough that the pointer leaves this card (or moves onto a
  // different one) before we return. If we blindly call .classList.remove
  // / .innerHTML now, the chip ends up showing stale metadata for a card
  // that's no longer hovered. Guard with the canonical hoveredId.
  if (state.heatTrail.hoveredId !== sid) return;
  if (!payload) {
    chip.classList.add("hidden");
    return;
  }
  // Enrich fields surfaced by P5 enrich.rs.
  const intent = enrichField(payload, "intent");
  const outcome = enrichField(payload, "outcome");
  const arc = enrichField(payload, "arc");
  const topic = enrichField(payload, "topic");
  // Pick 4 informative bits, fall back to title.
  const bits = [];
  if (arc) bits.push(`<span class="bit bit-arc">${escapeHtml(arc)}</span>`);
  if (outcome)
    bits.push(
      `<span class="bit bit-out bit-out-${outcomeClass(outcome)}">${escapeHtml(outcome)}</span>`,
    );
  if (intent) bits.push(`<span class="bit bit-intent">${escapeHtml(intent)}</span>`);
  if (topic) bits.push(`<span class="bit bit-topic">${escapeHtml(topic)}</span>`);
  if (!bits.length) {
    chip.classList.add("hidden");
    return;
  }
  chip.innerHTML = bits.join("");
  // Position next to the card top-right.
  const root = document.getElementById("results");
  const er = cardEl.getBoundingClientRect();
  const pr = root.getBoundingClientRect();
  chip.style.left = `${er.right - pr.left - 20}px`;
  chip.style.top = `${er.top - pr.top + 8}px`;
  chip.classList.remove("hidden");
}

function enrichField(payload, key) {
  if (!payload) return "";
  const v = payload[key];
  if (v == null) return "";
  if (typeof v === "string") return v;
  if (Array.isArray(v)) return v.slice(0, 2).join(", ");
  return String(v);
}

function outcomeClass(s) {
  const low = String(s).toLowerCase();
  if (low.includes("resolve") || low.includes("ok") || low.includes("success"))
    return "ok";
  if (low.includes("partial") || low.includes("wip")) return "partial";
  if (low.includes("unresolved") || low.includes("fail") || low.includes("error"))
    return "bad";
  return "neutral";
}

// ─────────────────────────────────────────────────────────────────────────────
// WOW-3: Lens contribution bars
// ─────────────────────────────────────────────────────────────────────────────

// Per-vector colors used by the stacked bar AND the heat-trail chips.
// Chosen for accessible-on-dark luminance + framework agnosticism.
const VEC_COLOR = {
  content: "oklch(72% 0.14 245)",
  tool: "oklch(75% 0.16 165)",
  path: "oklch(70% 0.14 295)",
  error: "oklch(70% 0.20 25)",
  code: "oklch(78% 0.14 95)",
  content_late: "oklch(70% 0.18 320)",
  path_sparse: "oklch(65% 0.10 285)",
  tool_sparse: "oklch(70% 0.12 175)",
};

function renderContributionBar(breakdown) {
  if (!breakdown) return "";
  const per = breakdown.per_vector || {};
  // Sum positive contributions only. Negative scores happen with cosine
  // and we visually treat them as zero (already a no-op below).
  let total = 0;
  const segs = [];
  for (const name of BAR_VECTORS) {
    const v = per[name];
    if (typeof v !== "number" || !Number.isFinite(v) || v <= 0) continue;
    segs.push([name, v]);
    total += v;
  }
  // Legacy/empty path: render a flat indeterminate bar so the card still has
  // visual weight.
  if (!segs.length || total <= 0) {
    return `
      <div class="contrib-bar flat" aria-hidden="true">
        <span class="contrib-seg flat" style="flex-basis:100%"></span>
      </div>`;
  }
  const segsHtml = segs
    .map(([name, v]) => {
      const pct = (v / total) * 100;
      const color = VEC_COLOR[name] || "oklch(70% 0.10 220)";
      return `<span
        class="contrib-seg"
        style="flex-basis:${pct.toFixed(2)}%; background:${color}"
        title="${name}  ${v.toFixed(3)}"
        data-vec="${name}"
      ></span>`;
    })
    .join("");
  const legend = segs
    .map(
      ([name, v]) => `
        <span class="contrib-key">
          <span class="contrib-dot" style="background:${VEC_COLOR[name] || "#fff"}"></span>
          ${name}<span class="contrib-num"> ${v.toFixed(2)}</span>
        </span>`,
    )
    .join("");
  return `
    <div class="contrib-bar" role="img" aria-label="Per-vector contributions: ${
      segs.map(([k, v]) => `${k} ${v.toFixed(2)}`).join(", ")
    }">${segsHtml}</div>
    <div class="contrib-legend">${legend}</div>`;
}

function renderScoreChips(breakdown) {
  if (!breakdown) return "";
  const r = breakdown.recency_factor;
  const e = breakdown.has_errors_boost;
  const hasR = typeof r === "number" && Number.isFinite(r);
  const hasE = typeof e === "number" && Number.isFinite(e);
  if (!hasR && !hasE) return "";
  const parts = [];
  if (hasR) parts.push(`<span class="chip-rec">recency <b>${r.toFixed(2)}</b></span>`);
  if (hasE && e > 0)
    parts.push(`<span class="chip-err">errors <b>+${e.toFixed(2)}</b></span>`);
  return `<div class="score-chips">${parts.join("")}</div>`;
}

async function selectSession(sessionId, opts = {}) {
  const silent = !!opts.silent;
  state.selected = sessionId;
  for (const c of document.querySelectorAll(".card, .stack-card")) {
    c.classList.toggle("selected", c.dataset.sessionId === sessionId);
  }
  const inspector = document.getElementById("inspector");
  if (!silent) {
    inspector.innerHTML = `<div class="empty">Loading ${sessionId.slice(0, 8)}…</div>`;
  }
  try {
    const payload = await invoke("get_session", { sessionId });
    if (!payload) {
      const summary = state.stack.find((s) => s.session_id === sessionId);
      if (summary) {
        renderInspector(summary, sessionId);
        schedulePrediction(sessionId);
        return;
      }
    }
    renderInspector(payload, sessionId);
    // Predictions always run when a session is selected — even from silent
    // stack navigation. The shimmer placeholder is cheap and signals
    // "we're computing what past-you did next" before the data lands.
    schedulePrediction(sessionId);
  } catch (err) {
    if (silent) return;
    inspector.innerHTML = `<div class="empty">Error: ${escapeHtml(String(err))}</div>`;
  }
}

// Debounce rapid arrow-key nav: only the last session the user lingers on
// for ~220 ms gets a prediction roundtrip.
let predictionTimer = null;
function schedulePrediction(sessionId) {
  if (predictionTimer) clearTimeout(predictionTimer);
  // Show shimmer immediately so the user sees the panel responding.
  renderPredictionPanel({ loading: true, source_session_id: sessionId });
  predictionTimer = setTimeout(() => {
    predictionTimer = null;
    loadPredictions(sessionId);
  }, 220);
}

// ---------------------------------------------------------------------------
// Path 2 — Predictive next-actions panel
// ---------------------------------------------------------------------------

const TOOL_ICON = {
  Bash: "🖥",
  Edit: "✏️",
  MultiEdit: "✏️",
  Write: "📝",
  Read: "📖",
  Grep: "🔎",
  Glob: "🔎",
  WebFetch: "🌐",
  WebSearch: "🌐",
  Task: "🤖",
  Agent: "🤖",
  TaskCreate: "📋",
  TaskUpdate: "📋",
  TaskList: "📋",
  NotebookEdit: "📓",
};

function toolIcon(name) {
  return TOOL_ICON[name] || "🛠";
}

async function loadPredictions(sessionId) {
  // Render a placeholder immediately so the user knows something's happening.
  renderPredictionPanel({
    loading: true,
    source_session_id: sessionId,
  });
  try {
    const ctx = await invoke("predict_next_actions", {
      sessionId,
      lastNTurns: 3,
      horizon: 3,
      neighbors: 8,
    });
    if (state.selected !== sessionId) return; // user clicked another card
    renderPredictionPanel(ctx);
  } catch (err) {
    if (state.selected !== sessionId) return;
    renderPredictionPanel({
      error: String(err),
      source_session_id: sessionId,
    });
  }
}

function renderPredictionPanel(ctx) {
  // The slot is rendered by renderInspector above the kvs/raw, so it's
  // always visible without scrolling. If the inspector hasn't rendered yet
  // (e.g., predictions arrived before payload), create a panel inline at
  // the inspector root and let the next inspector render replace it.
  let panel = document.getElementById("prediction-panel");
  if (!panel) {
    const inspector = document.getElementById("inspector");
    panel = document.createElement("section");
    panel.id = "prediction-panel";
    panel.className = "prediction-panel";
    inspector.appendChild(panel);
  }
  if (ctx.loading) {
    panel.innerHTML = `
      <header class="prediction-header">
        <h3>🔮 What past-you did next</h3>
        <span class="muted">Searching similar sessions…</span>
      </header>
      <div class="prediction-loading">
        <div class="shimmer"></div>
        <div class="shimmer"></div>
        <div class="shimmer"></div>
      </div>
    `;
    return;
  }
  if (ctx.error) {
    panel.innerHTML = `
      <header class="prediction-header">
        <h3>🔮 What past-you did next</h3>
      </header>
      <div class="empty">No prediction (${escapeHtml(ctx.error)})</div>
    `;
    return;
  }
  const preds = ctx.predictions || [];
  if (!preds.length) {
    panel.innerHTML = `
      <header class="prediction-header">
        <h3>🔮 What past-you did next</h3>
        <span class="muted">${ctx.neighbors_searched || 0} neighbor(s) inspected</span>
      </header>
      <div class="empty">No close-enough past sessions to project a next action from.</div>
    `;
    return;
  }
  panel.innerHTML = `
    <header class="prediction-header">
      <h3>🔮 What past-you did next</h3>
      <span class="muted">${ctx.neighbors_used || 0} of ${ctx.neighbors_searched || 0} neighbor(s) matched</span>
    </header>
    <div class="prediction-grid" id="prediction-grid"></div>
    <div class="prediction-list"></div>
  `;
  // WOW-4: render the cinematic 4×3 thumbnail grid above the per-tool list.
  renderPredictionGrid(panel, preds, ctx.source_session_id);
  const list = panel.querySelector(".prediction-list");
  for (const p of preds) {
    const freqPct = Math.round((p.frequency || 0) * 100);
    const confPct = Math.round((p.confidence || 0) * 100);
    const card = document.createElement("article");
    card.className = "prediction-card";
    card.innerHTML = `
      <div class="prediction-rank">${p.rank}</div>
      <div class="prediction-body">
        <div class="prediction-head">
          <span class="prediction-icon">${toolIcon(p.tool_name)}</span>
          <span class="prediction-tool">${escapeHtml(p.tool_name)}</span>
          <span class="prediction-meta">${freqPct}% of times · sim ${confPct}%</span>
        </div>
        <pre class="prediction-example">${escapeHtml(p.example_input_summary || "—")}</pre>
        <div class="prediction-source">
          <span class="legend-dot xs" style="background:${projectColor(p.from_session_project)}"></span>
          <span class="prediction-source-text">from <strong>${escapeHtml(p.from_session_project || "?")}</strong> · turn #${p.from_turn_index}</span>
          <button type="button" class="btn ghost xs" data-replay-id="${p.from_session_id}" data-replay-turn="${p.from_turn_index}">Jump to replay</button>
        </div>
      </div>
      <div class="prediction-freq-bar"><span style="width:${freqPct}%"></span></div>
    `;
    card
      .querySelector("button[data-replay-id]")
      .addEventListener("click", async (e) => {
        e.stopPropagation();
        const sid = e.target.dataset.replayId;
        const turn = parseInt(e.target.dataset.replayTurn, 10);
        await openReplay(sid, { project_name: p.from_session_project });
        // After turns load, jump to that index.
        setTimeout(() => {
          if (state.replay.sessionId === sid) {
            state.replay.cursor = Math.min(
              turn,
              state.replay.turns.length - 1,
            );
            renderReplayTurn(state.replay.cursor);
          }
        }, 600);
      });
    list.appendChild(card);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// WOW-4: Predict Cinematic Grid (4×3 thumbnails + view-transition zoom)
// ─────────────────────────────────────────────────────────────────────────────
// We reuse the prediction response: each prediction references a from_session
// that we treat as a "neighbor". Dedupe by session id so we don't show the
// same card twice when multiple tools predict from the same neighbor. Limit to
// 12 thumbnails. Click → fade/zoom into Replay via document.startViewTransition.

async function renderPredictionGrid(panel, preds, sourceSessionId) {
  const grid = panel.querySelector("#prediction-grid");
  if (!grid) return;
  // Group predictions by from_session_id so each thumbnail consolidates the
  // tools predicted from that neighbor.
  const byNeighbor = new Map();
  for (const p of preds) {
    const sid = p.from_session_id;
    if (!sid) continue;
    let row = byNeighbor.get(sid);
    if (!row) {
      row = {
        sid,
        project: p.from_session_project,
        tools: [],
        confidenceMax: 0,
      };
      byNeighbor.set(sid, row);
    }
    row.tools.push(p.tool_name);
    row.confidenceMax = Math.max(row.confidenceMax, p.confidence || 0);
  }
  const items = Array.from(byNeighbor.values()).slice(0, 12);
  if (!items.length) {
    grid.remove();
    return;
  }
  state.predictGrid = { sessionId: sourceSessionId, items };
  grid.innerHTML = "";
  // Fetch payloads in parallel so each thumbnail can show enrich.outcome,
  // enrich.arc, and a tool histogram derived from tool_counts (in payload).
  const payloads = await Promise.all(
    items.map((it) =>
      invoke("get_session", { sessionId: it.sid }).catch(() => null),
    ),
  );
  items.forEach((it, i) => {
    const p = payloads[i] || {};
    const title = p.ai_title || it.project || "(untitled)";
    const topic = enrichField(p, "topic");
    const outcome = enrichField(p, "outcome");
    const arc = enrichField(p, "arc");
    // Approximate "top 3 tools" by accumulating tool counts from the payload
    // (tool_counts), falling back to the predicted tool names if missing.
    const toolCounts = (p.tool_counts && typeof p.tool_counts === "object") ? p.tool_counts : null;
    let topTools = [];
    if (toolCounts) {
      topTools = Object.entries(toolCounts)
        .sort((a, b) => b[1] - a[1])
        .slice(0, 3);
    } else {
      const counts = new Map();
      for (const t of it.tools) counts.set(t, (counts.get(t) || 0) + 1);
      topTools = Array.from(counts.entries())
        .sort((a, b) => b[1] - a[1])
        .slice(0, 3);
    }
    const card = document.createElement("button");
    card.type = "button";
    card.className = `pred-thumb arc-${arcClass(arc)}`;
    // view-transition-name pairs the thumbnail with the Replay header.
    card.style.viewTransitionName = `card-${cssSafeId(it.sid)}`;
    card.dataset.sessionId = it.sid;
    const toolsHtml = topTools.length
      ? topTools
          .map(
            ([name, count]) =>
              `<span class="pt-tool">${escapeHtml(name)}<span class="pt-count">×${count}</span></span>`,
          )
          .join("")
      : "";
    const outcomeBadge = outcome
      ? `<span class="pt-outcome pt-outcome-${outcomeClass(outcome)}">${escapeHtml(outcome)}</span>`
      : "";
    card.innerHTML = `
      <header class="pt-head">
        <span class="pt-arc" title="${escapeHtml(arc || "")}">${arcIcon(arc)}</span>
        <span class="pt-proj">${escapeHtml(it.project || "?")}</span>
        ${outcomeBadge}
      </header>
      <h4 class="pt-title">${escapeHtml(title.slice(0, 80))}</h4>
      ${topic ? `<div class="pt-topic">${escapeHtml(topic)}</div>` : ""}
      <div class="pt-tools">${toolsHtml}</div>
      <footer class="pt-foot">
        <code class="pt-sid">${escapeHtml(it.sid.slice(0, 8))}…</code>
        <span class="pt-conf">${Math.round((it.confidenceMax || 0) * 100)}%</span>
      </footer>
    `;
    card.addEventListener("click", () => cinematicZoom(it.sid, p, card));
    card.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        cinematicZoom(it.sid, p, card);
      }
    });
    grid.appendChild(card);
  });
}

function arcClass(arc) {
  const s = String(arc || "").toLowerCase();
  if (s.includes("debug")) return "debug";
  if (s.includes("fix")) return "fix";
  if (s.includes("impl") || s.includes("build")) return "impl";
  if (s.includes("explor")) return "explore";
  return "mixed";
}
function arcIcon(arc) {
  switch (arcClass(arc)) {
    case "debug": return "🔬";
    case "fix": return "🛠";
    case "impl": return "🧱";
    case "explore": return "🧭";
    default: return "·";
  }
}

function cssSafeId(s) {
  return String(s).replace(/[^a-zA-Z0-9_-]/g, "_");
}

// WOW-4: cinematic zoom from a grid thumbnail into the Replay modal. Uses the
// View Transitions API where available; falls back to a 240ms opacity fade.
function cinematicZoom(sid, payload, cardEl) {
  // Tag the Replay modal with the same view-transition-name so the browser
  // pairs the start (thumbnail) with the end (replay header).
  const modal = document.getElementById("replay-modal");
  if (modal) {
    const headerEl = modal.querySelector("header");
    if (headerEl) {
      headerEl.style.viewTransitionName = `card-${cssSafeId(sid)}`;
      headerEl.dataset.transitionFor = sid;
    }
  }
  const doOpen = () => openReplay(sid, payload || { project_name: cardEl?.querySelector(".pt-proj")?.textContent });

  if (!REDUCED_MOTION.matches && document.startViewTransition) {
    document.startViewTransition(() => {
      doOpen();
    });
  } else {
    // Reduced-motion fallback: 240ms opacity blend. The replay modal already
    // animates via <dialog> showModal so this just keeps the spec promise.
    if (cardEl) {
      cardEl.style.transition = "opacity 240ms ease";
      cardEl.style.opacity = "0.4";
      setTimeout(() => {
        cardEl.style.opacity = "1";
      }, 260);
    }
    doOpen();
  }
}

function renderInspector(payload, sessionId) {
  const inspector = document.getElementById("inspector");
  if (!payload) {
    inspector.innerHTML = `<div class="empty">Session ${sessionId} not in index.</div>`;
    return;
  }
  const fields = [
    ["session_id", payload.session_id],
    ["project", payload.project_name],
    ["path", payload.project_path],
    ["branch", payload.git_branch],
    ["claude version", payload.claude_version],
    ["title", payload.ai_title || "(untitled)"],
    ["started", payload.start_iso],
    ["ended", payload.end_iso],
    ["user turns", payload.user_turns],
    ["assistant turns", payload.assistant_turns],
    ["tool calls", payload.tool_count],
    ["had errors", payload.has_errors],
  ];
  const rows = fields
    .map(
      ([k, v]) => `
      <div class="kv">
        <span class="k">${escapeHtml(k)}</span>
        <span class="v">${escapeHtml(String(v ?? ""))}</span>
      </div>`,
    )
    .join("");
  // Note the prediction-slot lives BEFORE the kvs / raw payload so it's
  // always visible without scrolling — it's the differentiating surface.
  inspector.innerHTML = `
    <header class="inspector-head">
      <h3>${escapeHtml(payload.project_name || "session")}</h3>
      <code>${escapeHtml(sessionId)}</code>
    </header>
    <section id="prediction-panel" class="prediction-panel"></section>
    <div class="kvs">${rows}</div>
    <details class="raw">
      <summary>Raw payload</summary>
      <pre>${escapeHtml(JSON.stringify(payload, null, 2))}</pre>
    </details>
  `;
}

async function onRecall() {
  const text = document.getElementById("recall-input").value.trim();
  if (!text) {
    setStatus("Paste an error message into the Recall box first.");
    return;
  }
  setStatus("Searching past errors…");
  try {
    const hits = await invoke("recall", { errorText: text, limit: 5 });
    renderResults(hits);
    setStatus(`Recall: ${hits.length} past session(s) match.`);
  } catch (err) {
    setStatus(`Recall failed: ${err}`);
  }
}

async function onTopology() {
  const modal = document.getElementById("topology-modal");
  modal.showModal();
  const canvas = document.getElementById("topology-canvas");
  // WOW-2: preserve the agent filter pill + gap overlay nodes when reloading.
  preserveTopologyChrome(canvas, () => {
    canvas.innerHTML = `<div class="empty">Computing MST…</div>`;
  });
  try {
    const topo = await invoke("topology", { sample: 80, perPoint: 6 });
    renderTopology3D(topo, canvas);
  } catch (err) {
    canvas.innerHTML = `<div class="empty">Topology failed: ${escapeHtml(String(err))}</div>`;
  }
}

// WOW-2: when we clear topology DOM to re-render, salvage the chrome
// (agent-filter + gap overlay containers) so we don't lose them or have to
// re-create them as a side-effect of every refresh.
function preserveTopologyChrome(host, mutate) {
  const filter = document.getElementById("agent-filter");
  const overlay = document.getElementById("gap-overlay");
  mutate();
  if (filter && !host.contains(filter)) host.appendChild(filter);
  if (overlay && !host.contains(overlay)) {
    overlay.innerHTML = "";
    host.appendChild(overlay);
  }
}

// WOW-2: agent filter pill — toggles which nodes are visible by source_agent.
// source_agent isn't carried by TopoNode, so we resolve it via get_session
// in a single batch the first time the modal is opened.
function attachAgentFilterHandlers() {
  for (const pill of document.querySelectorAll(".agent-pill")) {
    pill.addEventListener("click", () => {
      const agent = pill.dataset.agent;
      if (state.agentFilter === agent) return;
      state.agentFilter = agent;
      for (const p of document.querySelectorAll(".agent-pill")) {
        const isActive = p === pill;
        p.classList.toggle("active", isActive);
        p.setAttribute("aria-checked", isActive ? "true" : "false");
      }
      applyAgentFilter();
    });
  }
}

function applyAgentFilter() {
  if (!topologyGraph) return;
  const sel = state.agentFilter;
  topologyGraph.nodeVisibility((n) => {
    if (sel === "both") return true;
    const agent = state.agentByNode.get(n.id);
    if (!agent) return true; // unknown → show until lookup arrives
    return agent === sel;
  });
  // Single-agent mode: paint nodes uniformly to make the lens unmistakable.
  topologyGraph.nodeColor((n) => {
    const agent = state.agentByNode.get(n.id) || "claude_code";
    if (sel === "claude_code") return "oklch(74% 0.16 245)";
    if (sel === "codex") return "oklch(80% 0.15 75)";
    // Both: honor the existing project color (or dim when highlight active).
    if (highlightedProject && n.project !== highlightedProject) return dim(n.color);
    return n.color;
  });
  // Shape encoding — open circle for codex, filled for claude. We approximate
  // this on 3d-force-graph with nodeOpacity to keep the WebGL path light.
  topologyGraph.nodeOpacity(0.95);
  // Re-render link directional particles too so cross-project gateways stay
  // visible only between currently-visible nodes.
  const data = topologyGraph.graphData();
  topologyGraph.linkDirectionalParticles((l) => {
    if (sel !== "both") {
      const aOk = nodeVisible(l.source);
      const bOk = nodeVisible(l.target);
      if (!aOk || !bOk) return 0;
    }
    return l.cross ? 2 : 0;
  });
  // Touch the data ref so 3d-force-graph picks up the visibility change.
  topologyGraph.graphData(data);
}

function nodeVisible(nodeRef) {
  const id = typeof nodeRef === "object" ? nodeRef.id : nodeRef;
  if (state.agentFilter === "both") return true;
  const agent = state.agentByNode.get(id);
  if (!agent) return true;
  return agent === state.agentFilter;
}

// 3d-force-graph instance kept around so we can dispose it cleanly between
// modal openings (WebGL contexts are expensive to leak).
let topologyGraph = null;

function disposeTopology() {
  if (topologyGraph && typeof topologyGraph._destructor === "function") {
    try {
      topologyGraph._destructor();
    } catch {}
  }
  topologyGraph = null;
}

function renderTopology3D(topo, mount) {
  disposeTopology();
  // DEDUP FIX (Gemini PR #7 review, main.js:1465): the agent-filter +
  // gap-overlay salvage dance was inlined here, but the empty/no-engine
  // branches below already use the `preserveTopologyChrome` helper. Use
  // that same helper here for consistency and so future chrome additions
  // only need to be tracked in one place.
  preserveTopologyChrome(mount, () => {
    mount.innerHTML = "";
  });
  const { nodes, edges } = topo;
  const statsEl = document.getElementById("topology-stats");
  const legendEl = document.getElementById("topology-legend");
  if (statsEl) statsEl.innerHTML = "";
  if (legendEl) legendEl.innerHTML = "";

  if (!nodes.length) {
    // WOW-2: preserve the agent-filter + gap-overlay chrome on the empty path.
    preserveTopologyChrome(mount, () => {
      mount.innerHTML = `<div class="empty">No nodes yet — re-index first.</div>`;
    });
    return;
  }
  if (typeof window.ForceGraph3D !== "function") {
    preserveTopologyChrome(mount, () => {
      mount.innerHTML = `<div class="empty">3D engine failed to load (vendor/3d-force-graph.min.js).</div>`;
    });
    return;
  }

  // ---- Aggregate project metadata for legend + cluster force --------------
  const projects = new Map();
  for (const n of nodes) {
    const key = n.project_name || "?";
    let p = projects.get(key);
    if (!p) {
      p = {
        name: key,
        color: projectColor(key),
        count: 0,
        earliest: n.start_iso || "",
        latest: n.start_iso || "",
        sessionIds: new Set(),
      };
      projects.set(key, p);
    }
    p.count++;
    p.sessionIds.add(n.session_id);
    if (n.start_iso && (!p.earliest || n.start_iso < p.earliest)) p.earliest = n.start_iso;
    if (n.start_iso && (!p.latest || n.start_iso > p.latest)) p.latest = n.start_iso;
  }
  // Backend-provided auto-labels (A) keyed by project name.
  const insightByName = new Map();
  for (const ins of topo.project_insights || []) {
    insightByName.set(ins.project_name, ins);
    const p = projects.get(ins.project_name);
    if (p) p.insight = ins;
  }
  const projectList = Array.from(projects.values()).sort((a, b) => b.count - a.count);

  // Count cross-project edges = "ideas that bridged your work".
  const nodeProject = new Map(nodes.map((n) => [n.session_id, n.project_name || "?"]));
  let bridges = 0;
  for (const e of edges) {
    if (nodeProject.get(e.a) !== nodeProject.get(e.b)) bridges++;
  }

  // ---- Graph data ---------------------------------------------------------
  const graphData = {
    nodes: nodes.map((n) => ({
      id: n.session_id,
      project: n.project_name || "?",
      title: n.ai_title || "(untitled)",
      start_iso: n.start_iso || "",
      user_turns: n.user_turns || 0,
      tool_count: n.tool_count || 0,
      color: projectColor(n.project_name),
      val: Math.max(1, Math.sqrt((n.user_turns || 0) + 1)),
    })),
    links: edges.map((e) => {
      const isCross = nodeProject.get(e.a) !== nodeProject.get(e.b);
      return {
        source: e.a,
        target: e.b,
        similarity: Math.max(0, Math.min(1, 1 - e.distance)),
        cross: isCross,
      };
    }),
  };

  // ---- Build 3D scene -----------------------------------------------------
  const G = window.ForceGraph3D({
    controlType: "orbit",
    backgroundColor: "#16161a",
  })(mount)
    .graphData(graphData)
    .nodeRelSize(5)
    .nodeVal((n) => n.val)
    .nodeColor((n) => (highlightedProject && n.project !== highlightedProject ? dim(n.color) : n.color))
    .nodeOpacity(0.95)
    .nodeResolution(16)
    .nodeLabel((n) => topologyTooltip(n))
    // WOW-2: cross-project bridges in violet (oklch purple) = a gateway
    // between idea-clusters. linkDirectionalParticles=2 (built-in 3d-force-graph
    // particles) emphasizes the direction without custom shaders.
    .linkColor((l) =>
      l.cross
        ? `oklch(75% 0.20 290 / ${(0.55 + l.similarity * 0.40).toFixed(3)})`
        : `rgba(10, 132, 255, ${0.30 + l.similarity * 0.45})`,
    )
    .linkOpacity(1)
    .linkWidth((l) => (l.cross ? 1.4 + l.similarity * 2.5 : 0.4 + l.similarity * 1.6))
    .linkDirectionalParticles((l) => (l.cross ? 2 : 0))
    .linkDirectionalParticleWidth(1.2)
    .linkDirectionalParticleColor(() => "oklch(80% 0.18 295)")
    .onNodeClick((node) => {
      document.getElementById("topology-modal").close();
      selectSession(node.id);
    })
    .onNodeHover((node) => {
      mount.style.cursor = node ? "pointer" : "default";
    });

  // Per-link target distance — similar pairs attract tighter.
  G.d3Force("link").distance((l) => 60 + (1 - l.similarity) * 220);
  G.d3Force("charge").strength(-90);
  // Custom clustering force: pull same-project nodes toward their group centroid.
  G.d3Force("project-cluster", projectClusterForce(0.06));
  setTimeout(() => G.zoomToFit(900, 80), 1400);

  topologyGraph = G;

  // WOW-2: kick off background source_agent fetch, then re-apply the agent
  // filter when results land. Batches via a small concurrency pool so we
  // don't fire 10k get_session calls at once on large corpora.
  hydrateAgentMetadata(nodes).then(() => {
    if (topologyGraph === G) applyAgentFilter();
  });
  // Set up the gap overlay so insight HTML divs follow the 3D camera. The
  // rAF loop self-cancels when the modal closes (disposeTopology nulls G).
  startGapOverlayLoop(G, projectList, topo.gap_insights || []);

  // ---- Stats bar + legend -------------------------------------------------
  if (statsEl) {
    statsEl.innerHTML = `
      <div class="topology-stat">
        <span class="stat-num">${projectList.length}</span>
        <span class="stat-label">projects</span>
      </div>
      <div class="topology-stat">
        <span class="stat-num">${nodes.length}</span>
        <span class="stat-label">sessions</span>
      </div>
      <div class="topology-stat">
        <span class="stat-num">${bridges}</span>
        <span class="stat-label">bridges</span>
      </div>
    `;
  }

  if (legendEl) {
    legendEl.innerHTML = "";
    const heading = document.createElement("div");
    heading.className = "legend-heading";
    heading.textContent = "Projects";
    legendEl.appendChild(heading);

    for (const p of projectList) {
      const row = document.createElement("button");
      row.type = "button";
      row.className = "legend-row";
      row.dataset.project = p.name;
      const earliest = p.earliest.slice(0, 10);
      const latest = p.latest.slice(0, 10);
      const range = earliest === latest ? earliest : `${earliest} → ${latest}`;
      const ins = p.insight;
      const themeChip = ins
        ? `<span class="theme-chip">${escapeHtml(ins.theme)}</span>`
        : "";
      const toolsBreakdown = ins?.top_tools?.length
        ? ins.top_tools
            .slice(0, 3)
            .map(
              ([name, count]) =>
                `<span class="tool-chip">${escapeHtml(name)}<span class="tool-count">×${count}</span></span>`,
            )
            .join("")
        : "";
      const bridgeIcon = ins?.bridges_out
        ? `<span class="bridge-pill" title="${ins.bridges_out} cross-project bridges">↔ ${ins.bridges_out}</span>`
        : `<span class="bridge-pill isolated" title="No bridges to other projects">isolated</span>`;
      const errPill = ins?.had_errors
        ? `<span class="err-pill" title="${ins.had_errors} session(s) had tool errors">errors ${ins.had_errors}</span>`
        : "";
      row.innerHTML = `
        <div class="legend-row-head">
          <span class="legend-dot" style="background:${p.color}"></span>
          <span class="legend-name">${escapeHtml(p.name)}</span>
          <span class="legend-count">${p.count}</span>
        </div>
        ${themeChip ? `<div class="legend-row-theme">${themeChip}${bridgeIcon}${errPill}</div>` : ""}
        ${toolsBreakdown ? `<div class="legend-row-tools">${toolsBreakdown}</div>` : ""}
        <div class="legend-row-foot">${range}</div>
      `;
      row.addEventListener("mouseenter", () => setHighlight(p.name));
      row.addEventListener("mouseleave", () => setHighlight(null));
      row.addEventListener("click", () => focusCluster(p.name));
      legendEl.appendChild(row);
    }

    // Gap analysis (C) — "what's missing" panel
    const gaps = topo.gap_insights || [];
    if (gaps.length) {
      const gapHeading = document.createElement("div");
      gapHeading.className = "legend-heading gap-heading";
      gapHeading.innerHTML = `⚡ Gaps <span class="muted">(${gaps.length})</span>`;
      legendEl.appendChild(gapHeading);

      for (const g of gaps) {
        const card = document.createElement("div");
        card.className = `gap-card gap-${g.kind}`;
        const kindLabel = g.kind === "isolated" ? "isolated" : "near miss";
        const simBadge = g.similarity
          ? `<span class="sim-badge">sim ${g.similarity.toFixed(2)}</span>`
          : "";
        const dotA = `<span class="legend-dot xs" style="background:${projectColor(g.project_a)}"></span>`;
        const dotB = g.project_b
          ? `<span class="legend-dot xs" style="background:${projectColor(g.project_b)}"></span>`
          : "";
        const projHead = g.project_b
          ? `${dotA}<strong>${escapeHtml(g.project_a)}</strong> &harr; ${dotB}<strong>${escapeHtml(g.project_b)}</strong>`
          : `${dotA}<strong>${escapeHtml(g.project_a)}</strong>`;
        card.innerHTML = `
          <div class="gap-head">
            <span class="gap-kind ${g.kind}">${kindLabel}</span>
            ${simBadge}
          </div>
          <div class="gap-proj">${projHead}</div>
          <div class="gap-msg">${escapeHtml(g.message)}</div>
          ${
            g.project_b
              ? `<button type="button" class="btn ghost xs gap-explore" data-a="${escapeHtml(g.project_a)}" data-b="${escapeHtml(g.project_b)}">Explore both</button>`
              : `<button type="button" class="btn ghost xs gap-explore" data-a="${escapeHtml(g.project_a)}">Focus cluster</button>`
          }
        `;
        const btn = card.querySelector(".gap-explore");
        btn.addEventListener("click", () => {
          if (g.project_b) {
            // Highlight both projects simultaneously, zoom to their union.
            focusClusters([g.project_a, g.project_b]);
          } else {
            focusCluster(g.project_a);
          }
        });
        legendEl.appendChild(card);
      }
    }

    const explainer = document.createElement("div");
    explainer.className = "legend-explainer";
    explainer.innerHTML = `
      <div class="legend-mini">
        <span class="legend-line in-project"></span>
        in-project edge
      </div>
      <div class="legend-mini">
        <span class="legend-line cross-project"></span>
        cross-project bridge
      </div>
      <p>Hover a project row to highlight its cluster. Click to zoom.</p>
    `;
    legendEl.appendChild(explainer);
  }
}

// ---- Highlight + focus -----------------------------------------------------

let highlightedProject = null;

function setHighlight(project) {
  highlightedProject = project;
  if (!topologyGraph) return;
  // Re-trigger node colorization by re-applying the color fn.
  topologyGraph.nodeColor((n) =>
    highlightedProject && n.project !== highlightedProject ? dim(n.color) : n.color,
  );
  // Highlight legend row.
  document.querySelectorAll(".legend-row").forEach((row) => {
    row.classList.toggle("active", row.dataset.project === project);
  });
}

function focusCluster(project) {
  if (!topologyGraph) return;
  const ids = new Set(
    topologyGraph
      .graphData()
      .nodes.filter((n) => n.project === project)
      .map((n) => n.id),
  );
  topologyGraph.zoomToFit(900, 60, (n) => ids.has(n.id));
}

function focusClusters(projectNames) {
  if (!topologyGraph) return;
  const set = new Set(projectNames);
  const ids = new Set(
    topologyGraph
      .graphData()
      .nodes.filter((n) => set.has(n.project))
      .map((n) => n.id),
  );
  topologyGraph.zoomToFit(900, 60, (n) => ids.has(n.id));
  // Dim everything outside the focused pair.
  highlightedProject = null;
  topologyGraph.nodeColor((n) =>
    set.has(n.project) ? n.color : dim(n.color),
  );
}

function dim(hex) {
  // Convert "#rrggbb" to muted rgba — perceived 22% alpha.
  if (!hex || hex[0] !== "#" || hex.length < 7) return "rgba(255,255,255,0.15)";
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r},${g},${b},0.18)`;
}

function topologyTooltip(n) {
  const ts = (n.start_iso || "").slice(0, 16).replace("T", " ");
  return `
    <div style="font-family:-apple-system,sans-serif;font-size:12px;
                background:rgba(20,20,22,0.94);color:#fff;
                padding:8px 11px;border-radius:8px;
                border:1px solid rgba(255,255,255,0.18);
                box-shadow:0 8px 28px rgba(0,0,0,0.55);
                max-width:300px;line-height:1.4">
      <strong style="color:${n.color}">${escapeHtml(n.project)}</strong><br/>
      <span style="color:rgba(255,255,255,0.8)">${escapeHtml(n.title)}</span><br/>
      <span style="font-family:ui-monospace,monospace;font-size:10.5px;
                   color:rgba(255,255,255,0.55)">
        ${ts} · ${n.user_turns} user · ${n.tool_count} tools
      </span>
    </div>`;
}

// Custom d3 force pulling same-project nodes toward their group centroid.
// Combined with the link-distance force this produces clean per-project
// bubbles connected by yellow "bridge" edges where ideas crossed over.
function projectClusterForce(strength) {
  let nodes = [];
  function force(alpha) {
    const centroids = new Map();
    for (const n of nodes) {
      let c = centroids.get(n.project);
      if (!c) {
        c = { x: 0, y: 0, z: 0, count: 0 };
        centroids.set(n.project, c);
      }
      c.x += n.x || 0;
      c.y += n.y || 0;
      c.z += n.z || 0;
      c.count++;
    }
    for (const c of centroids.values()) {
      c.x /= c.count;
      c.y /= c.count;
      c.z /= c.count;
    }
    for (const n of nodes) {
      const c = centroids.get(n.project);
      if (!c) continue;
      n.vx = (n.vx || 0) + (c.x - n.x) * strength * alpha;
      n.vy = (n.vy || 0) + (c.y - n.y) * strength * alpha;
      n.vz = (n.vz || 0) + (c.z - n.z) * strength * alpha;
    }
  }
  force.initialize = (_nodes) => {
    nodes = _nodes;
  };
  return force;
}

// WOW-2: small async pool that hydrates `source_agent` for each topology node
// without paying a full N-roundtrip latency. 6 in flight is comfortable for
// macOS Tauri IPC + qdrant on localhost.
//
// MAGIC NUMBER FIX (Gemini PR #7 review, main.js:1844): the concurrency
// cap is named via this module-level constant so it can be tuned without
// hunting through code. Bumping it >8 risks saturating Tauri's IPC queue
// on big-corpus topology views; <4 noticeably slows hydration.
const HYDRATE_AGENT_WORKERS = 6;

async function hydrateAgentMetadata(nodes) {
  const PARALLEL = HYDRATE_AGENT_WORKERS;
  let i = 0;
  async function worker() {
    while (i < nodes.length) {
      const idx = i++;
      const sid = nodes[idx].session_id;
      if (state.agentByNode.has(sid)) continue;
      try {
        const p = await invoke("get_session", { sessionId: sid });
        const agent = (p && (p.source_agent || p["source_agent"])) || "claude_code";
        state.agentByNode.set(sid, agent);
      } catch {
        // Default to claude_code so unknowns don't disappear from the view.
        state.agentByNode.set(sid, "claude_code");
      }
    }
  }
  const workers = [];
  for (let k = 0; k < PARALLEL; k++) workers.push(worker());
  await Promise.all(workers);
}

// WOW-2: position gap-insight HTML overlays over the 3D-force-graph canvas
// using its `graph2ScreenCoords` projection. One requestAnimationFrame loop
// per topology open; auto-cancels when topologyGraph is reset.
function startGapOverlayLoop(G, projectList, gapInsights) {
  const overlay = document.getElementById("gap-overlay");
  if (!overlay) return;
  overlay.innerHTML = "";
  // Build the cards once; positioning happens in the rAF loop.
  // Centroid for each project = average position of its nodes (computed
  // every frame so dragging works).
  const cards = [];
  for (const g of gapInsights.slice(0, 4)) {
    const card = document.createElement("div");
    card.className = `gap-overlay-card gap-${g.kind}`;
    card.innerHTML = `
      <span class="gap-overlay-kind">${escapeHtml(g.kind)}</span>
      <span class="gap-overlay-msg">${escapeHtml(g.message)}</span>
    `;
    overlay.appendChild(card);
    cards.push({ el: card, a: g.project_a, b: g.project_b });
  }
  // Project centroid cache (recomputed each frame).
  function projectCentroid(name) {
    if (!G || !name) return null;
    let sx = 0, sy = 0, sz = 0, n = 0;
    for (const node of G.graphData().nodes) {
      if (node.project !== name) continue;
      sx += node.x || 0;
      sy += node.y || 0;
      sz += node.z || 0;
      n++;
    }
    if (!n) return null;
    return { x: sx / n, y: sy / n, z: sz / n };
  }
  function frame() {
    if (topologyGraph !== G) return; // disposed
    if (REDUCED_MOTION.matches) {
      // Reduced motion: pin to top-left in the canvas, no animation.
      cards.forEach((c, i) => {
        c.el.style.transform = `translate(12px, ${12 + i * 56}px)`;
      });
      requestAnimationFrame(frame);
      return;
    }
    for (const c of cards) {
      const ca = projectCentroid(c.a);
      const cb = c.b ? projectCentroid(c.b) : null;
      let mx, my, mz;
      if (ca && cb) {
        mx = (ca.x + cb.x) / 2;
        my = (ca.y + cb.y) / 2;
        mz = (ca.z + cb.z) / 2;
      } else if (ca) {
        mx = ca.x; my = ca.y; mz = ca.z;
      } else {
        c.el.style.opacity = "0";
        continue;
      }
      let screen;
      try {
        screen = G.graph2ScreenCoords(mx, my, mz);
      } catch {
        screen = null;
      }
      if (!screen) {
        c.el.style.opacity = "0";
        continue;
      }
      c.el.style.opacity = "1";
      c.el.style.transform = `translate(${(screen.x | 0)}px, ${(screen.y | 0)}px) translate(-50%, -50%)`;
    }
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
}

const COLOR_PALETTE = [
  "#ff9f0a", "#bf5af2", "#30d158", "#ff375f", "#ffd60a",
  "#0a84ff", "#ff6b35", "#af52de", "#66d4cf", "#ffb340",
  "#5e5ce6", "#ff453a", "#d8e056", "#4cc9b3", "#5ac8fa",
];

function projectColor(name) {
  if (!name) return "#888";
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) | 0;
  return COLOR_PALETTE[Math.abs(h) % COLOR_PALETTE.length];
}

function openMixModal() {
  renderMixDropzones();
  document.getElementById("mix-results").innerHTML = "";
  // Clear last picker state — but seed it with the most recent lens query
  // so the picker arrives pre-filled with sessions the user just looked at.
  const pickerInput = document.getElementById("mix-picker-input");
  const pickerResults = document.getElementById("mix-picker-results");
  if (pickerInput) {
    pickerInput.value = state.query || "";
  }
  if (pickerResults) {
    pickerResults.innerHTML = "";
  }
  // WOW-5: hydrate the target field with the focused stack card.
  const tgt = document.getElementById("mix-target");
  if (tgt && !tgt.value && state.stack[state.stackFocus]) {
    tgt.value = state.stack[state.stackFocus].session_id;
    state.mix.target = tgt.value;
  }
  document.getElementById("mix-modal").showModal();
  // If the user already had a non-empty query, run it once automatically so
  // the picker isn't empty on first open.
  if (pickerInput && pickerInput.value.trim()) {
    runMixPickerSearch();
  }
  // Defer the canvas init until after the dialog has its final size.
  requestAnimationFrame(() => initHyperplane());
  updateRunMixButton();
}

// UX FIX (D-13 review): self-contained picker so the modal doesn't depend on
// stack cards that the modal backdrop blocks.
//
// runMixPickerSearch() — invoke lens_search or list_sessions to populate the
// picker's result list. Each result row carries [+ pos] [− neg] buttons that
// call addToMix() the same way the main-view stack-card buttons do.
async function runMixPickerSearch() {
  const input = document.getElementById("mix-picker-input");
  const results = document.getElementById("mix-picker-results");
  if (!input || !results) return;
  const q = input.value.trim();
  if (!q) {
    results.innerHTML = '<p class="mix-picker-empty">Type a query above and press ↵, or paste a session_id.</p>';
    return;
  }
  // If the query looks like a session_id UUID, treat it as a direct id pick.
  const uuidRe = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  if (uuidRe.test(q)) {
    results.innerHTML = "";
    renderMixPickerRow(
      { session_id: q, project_name: "(by id)", ai_title: q, start_iso: "" },
      results,
    );
    return;
  }
  // Otherwise: lens search against the live collection.
  results.innerHTML = '<p class="mix-picker-empty">Searching…</p>';
  let hits = [];
  try {
    const weights = lensWeightsForV2();
    try {
      const raw = await invoke("lens_search_v2", { query: q, weights, limit: 12 });
      hits = (raw || []).map(normalizeLensResult);
    } catch {
      const legacy = { ...weights };
      delete legacy.content_late;
      delete legacy.diversity;
      delete legacy.fusion;
      const raw = await invoke("lens_search", { query: q, weights: legacy, limit: 12 });
      hits = (raw || []).map(legacyHitToLensResult);
    }
  } catch (err) {
    results.innerHTML = `<p class="mix-picker-empty">Search failed: ${escapeHtml(String(err))}</p>`;
    return;
  }
  if (!hits.length) {
    results.innerHTML = '<p class="mix-picker-empty">No matches.</p>';
    return;
  }
  results.innerHTML = "";
  for (const h of hits) {
    renderMixPickerRow(h, results);
  }
}

function renderMixPickerRow(hit, parent) {
  const row = document.createElement("div");
  row.className = "mix-picker-row";
  const meta = document.createElement("div");
  meta.className = "mix-picker-meta";
  const project = hit.project_name || "(unknown project)";
  const title = (hit.ai_title || "").trim() || "(untitled)";
  const start = hit.start_iso ? hit.start_iso.slice(0, 16).replace("T", " ") : "";
  meta.innerHTML = `
    <span class="mix-picker-project">${escapeHtml(project)}</span>
    <span class="mix-picker-title">${escapeHtml(title.slice(0, 80))}</span>
    <span class="mix-picker-start">${escapeHtml(start)}</span>
  `;
  const actions = document.createElement("div");
  actions.className = "mix-picker-actions";
  const posBtn = document.createElement("button");
  posBtn.type = "button";
  posBtn.className = "btn ghost xs";
  posBtn.textContent = "+ pos";
  posBtn.title = "Add as positive anchor";
  posBtn.addEventListener("click", () => {
    addToMix("positive", hit.session_id);
    posBtn.disabled = true;
    posBtn.textContent = "✓ pos";
  });
  const negBtn = document.createElement("button");
  negBtn.type = "button";
  negBtn.className = "btn ghost xs";
  negBtn.textContent = "− neg";
  negBtn.title = "Add as negative (anti-context) anchor";
  negBtn.addEventListener("click", () => {
    addToMix("negative", hit.session_id);
    negBtn.disabled = true;
    negBtn.textContent = "✓ neg";
  });
  actions.append(posBtn, negBtn);
  row.append(meta, actions);
  parent.appendChild(row);
}

function attachMixPickerEvents() {
  const input = document.getElementById("mix-picker-input");
  if (!input) return;
  let debounce = null;
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      runMixPickerSearch();
    }
  });
  input.addEventListener("input", () => {
    if (debounce) clearTimeout(debounce);
    debounce = setTimeout(() => {
      // Auto-search on quiet pause if the user typed at least 2 chars.
      if (input.value.trim().length >= 2) {
        runMixPickerSearch();
      }
    }, 350);
  });
}

// Keeps the [Run discovery] button + hint message in sync with state.
function updateRunMixButton() {
  const btn = document.getElementById("btn-run-mix");
  if (!btn) return;
  const ready =
    state.mix.positive.length > 0 || state.mix.negative.length > 0;
  btn.disabled = !ready;
  btn.title = ready
    ? "Run Qdrant Discovery on your selections"
    : "Add at least one positive OR negative session first";
}

function addToMix(side, sessionId) {
  if (!state.mix[side].includes(sessionId)) {
    state.mix[side].push(sessionId);
  }
  renderMixDropzones();
  updateRunMixButton();
}

function removeFromMix(side, sessionId) {
  state.mix[side] = state.mix[side].filter((s) => s !== sessionId);
  renderMixDropzones();
  updateRunMixButton();
  // Also flip the corresponding picker row's button back to its un-added
  // state if it's still on screen — search again to refresh.
  const input = document.getElementById("mix-picker-input");
  if (input && input.value.trim()) {
    runMixPickerSearch();
  }
}

function renderMixDropzones() {
  for (const side of ["positive", "negative"]) {
    const root = document.getElementById(`mix-${side}`);
    root.innerHTML = "";
    if (!state.mix[side].length) {
      const hint = document.createElement("span");
      hint.className = "dropzone-hint";
      hint.textContent =
        side === "positive"
          ? "search above OR click + pos on a card behind the modal…"
          : "search above OR click − neg on a card behind the modal…";
      root.appendChild(hint);
      continue;
    }
    for (const sid of state.mix[side]) {
      const chip = document.createElement("span");
      chip.className = "chip";
      chip.textContent = sid.slice(0, 8) + "…";
      const close = document.createElement("button");
      close.type = "button";
      close.textContent = "×";
      close.className = "chip-close";
      close.addEventListener("click", () => removeFromMix(side, sid));
      chip.appendChild(close);
      root.appendChild(chip);
    }
  }
}

async function runMix() {
  if (!state.mix.positive.length && !state.mix.negative.length) {
    document.getElementById("mix-results").textContent =
      "Add at least one positive or negative session first.";
    return;
  }
  const out = document.getElementById("mix-results");
  out.textContent = "Running discovery…";
  // WOW-5: prefer the pair-based Discovery API (P4 KB-03) when we have a
  // target. We turn each `positive` (and each `negative` paired with the
  // *first* positive as anchor) into a ContextPair.
  const tgtInput = document.getElementById("mix-target");
  const target = (tgtInput && tgtInput.value.trim()) || state.mix.target || null;
  state.mix.target = target;
  try {
    let hits;
    if (target && state.mix.positive.length >= 1) {
      // FUNCTIONAL FIX (Codex PR #7 review, main.js:2031): the backend
      // `ContextPair` deserializer (retrieval.rs::ContextPair) expects
      // snake_case `positive_session_id` / `negative_session_id` fields.
      // Sending `{positive, negative}` made every `mix_match_with_pairs`
      // call fail at the deserialization boundary, so this code path
      // ALWAYS silently fell back to legacy `mix_match`, meaning the
      // Hyperplane modal's "target session pair" semantics never reached
      // the server — which broke the WOW-5 Act IV demo storyline.
      const anchor = state.mix.positive[0];
      const pairs = [];
      for (const p of state.mix.positive.slice(1)) {
        pairs.push({ positive_session_id: p, negative_session_id: anchor });
      }
      for (const n of state.mix.negative) {
        pairs.push({ positive_session_id: anchor, negative_session_id: n });
      }
      if (!pairs.length) {
        pairs.push({ positive_session_id: anchor, negative_session_id: anchor });
      }
      try {
        hits = await invoke("mix_match_with_pairs", {
          targetSessionId: target,
          pairs,
          limit: 10,
        });
      } catch (pairErr) {
        // Fall back to the legacy positive/negative API.
        console.warn("mix_match_with_pairs failed, falling back:", pairErr);
        hits = await invoke("mix_match", {
          positive: state.mix.positive,
          negative: state.mix.negative,
          limit: 10,
        });
      }
    } else {
      hits = await invoke("mix_match", {
        positive: state.mix.positive,
        negative: state.mix.negative,
        limit: 10,
      });
    }
    state.mix.lastHits = hits || [];
    renderMixResults(out, hits || []);
    // WOW-5: render the resulting set as flying particles on the hyperplane.
    paintHyperplaneResults(hits || []);
  } catch (err) {
    out.textContent = `Mix failed: ${err}`;
  }
}

function renderMixResults(host, hits) {
  if (!hits.length) {
    host.textContent = "No discovery hits.";
    return;
  }
  host.innerHTML = "<h4 class=\"mix-results-title\">Discovered</h4>";
  for (const h of hits) {
    const row = document.createElement("div");
    row.className = "mix-row";
    const score = (h.score ?? 0).toFixed(3);
    row.innerHTML = `
      <div class="mix-row-head">
        <span class="mix-score">${score}</span>
        <span class="mix-proj">${escapeHtml(h.project_name || "?")}</span>
        <code class="mix-sid">${escapeHtml(h.session_id.slice(0, 12))}…</code>
      </div>
      <div class="mix-row-title">${escapeHtml(h.ai_title || "(untitled)")}</div>
      <div class="mix-row-actions">
        <button type="button" class="btn xs" data-act="up" data-sid="${escapeHtml(h.session_id)}">👍 more like this</button>
        <button type="button" class="btn xs" data-act="down" data-sid="${escapeHtml(h.session_id)}">👎 less</button>
        <button type="button" class="btn xs" data-act="replay" data-sid="${escapeHtml(h.session_id)}">Replay</button>
      </div>
    `;
    row.querySelectorAll("button[data-act]").forEach((btn) =>
      btn.addEventListener("click", (e) => {
        e.stopPropagation();
        const sid = btn.dataset.sid;
        const act = btn.dataset.act;
        if (act === "replay") {
          openReplay(sid, h);
        } else {
          applyRelevanceFeedback(sid, act === "up", host);
        }
      }),
    );
    host.appendChild(row);
  }
}

// WOW-5: KA-04 — re-rank current results by binary feedback on a row.
async function applyRelevanceFeedback(sessionId, isPositive, host) {
  const query = state.mix.lastQuery || state.query || "";
  if (!query) {
    setStatus("Type a query first for the relevance feedback to attach to.");
    return;
  }
  try {
    const pos = isPositive ? [sessionId] : [];
    const neg = isPositive ? [] : [sessionId];
    const re = await invoke("relevance_feedback", {
      previousQuery: query,
      positiveIds: pos,
      negativeIds: neg,
      limit: 10,
    });
    if (re && re.length) {
      const projected = re.map(legacyHitToLensResult);
      renderMixResults(host, projected);
      paintHyperplaneResults(projected);
    }
  } catch (err) {
    setStatus(`Relevance feedback failed: ${err}`);
  }
}

async function onSnapshot() {
  const path = prompt(
    "Snapshot destination path:",
    `/tmp/memex-${new Date().toISOString().slice(0, 10)}.snapshot`,
  );
  if (!path) return;
  setStatus("Exporting snapshot…");
  try {
    const name = await invoke("snapshot_export", { path });
    setStatus(`Snapshot '${name}' → ${path}`);
  } catch (err) {
    setStatus(`Snapshot failed: ${err}`);
  }
}

async function onRefresh() {
  setStatus("Re-indexing ~/.claude/projects…");
  try {
    const r = await invoke("refresh_index");
    state.collectionPoints = r.indexed;
    const dup = r.duplicates_skipped
      ? ` (${r.duplicates_skipped} duplicate sessionId(s) skipped)`
      : "";
    const errs = r.errors ? ` · ${r.errors} error(s)` : "";
    setStatus(`Re-indexed ${r.indexed}/${r.total_scanned} sessions${dup}${errs}.`);
    document.getElementById("collection-info").textContent = `· ${r.indexed} sessions`;
  } catch (err) {
    setStatus(`Re-index failed: ${err}`);
  }
}

function setStatus(msg) {
  document.getElementById("status-text").textContent = msg;
}

function setLatency(ms) {
  document.getElementById("status-latency").textContent = ms ? `${ms} ms` : "";
}

function debounce(fn, wait) {
  let t = null;
  return (...args) => {
    if (t) clearTimeout(t);
    t = setTimeout(() => fn(...args), wait);
  };
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c],
  );
}

// ─────────────────────────────────────────────────────────────────────────────
// WOW-5: Discovery Hyperplane (vanilla 2D canvas, no Three.js)
// ─────────────────────────────────────────────────────────────────────────────
// We don't have direct access to per-session embeddings from the frontend; we
// approximate a visual hyperplane by laying out positives on one side, negatives
// on the other, and emitting result cards from the positive side. Plane angle
// is derived from the anchor counts so it moves visually with the user's
// inputs.

const HYPERPLANE_FPS_BUDGET_MS = 16.67;

function initHyperplane() {
  const canvas = document.getElementById("hyperplane-canvas");
  if (!canvas) return;
  // Resize the canvas to its actual CSS box for sharp lines on HiDPI.
  const rect = canvas.getBoundingClientRect();
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  canvas.width = Math.max(640, rect.width | 0) * dpr;
  canvas.height = Math.max(280, rect.height | 0) * dpr;
  const ctx = canvas.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  // Cancel any prior loop.
  if (state.hyperplane.raf) cancelAnimationFrame(state.hyperplane.raf);
  state.hyperplane.particles = particlesFor(state.mix.lastHits || []);
  state.hyperplane.t0 = performance.now();
  loopHyperplane();
}

function loopHyperplane() {
  const canvas = document.getElementById("hyperplane-canvas");
  if (!canvas) return;
  const ctx = canvas.getContext("2d");
  const W = canvas.clientWidth || canvas.width;
  const H = canvas.clientHeight || canvas.height;
  const now = performance.now();
  const elapsed = (now - state.hyperplane.t0) / 1000;
  // Plane angle slowly drifts unless reduced-motion is set, so the scene
  // feels alive even when the user just opened it.
  const angle = REDUCED_MOTION.matches ? 0 : Math.sin(elapsed * 0.18) * 0.25;
  drawHyperplane(ctx, W, H, angle);
  // 60fps budget guard — if we slip, drop the drift animation.
  const frameStart = now;
  // Schedule next frame.
  state.hyperplane.raf = requestAnimationFrame(() => {
    const drawMs = performance.now() - frameStart;
    if (drawMs > HYPERPLANE_FPS_BUDGET_MS * 2) {
      // Bail out of the drift animation to stay snappy on slow machines.
      state.hyperplane.particles = (state.hyperplane.particles || []).slice(0, 10);
    }
    loopHyperplane();
  });
}

function drawHyperplane(ctx, W, H, planeAngle) {
  // HiDPI FIX (Gemini PR #7 review, main.js:2239): on retina displays the
  // canvas backing store is scaled by devicePixelRatio while W/H are
  // logical CSS pixels. Clearing only (0,0,W,H) leaves a stripe of the
  // previous frame visible along the right/bottom edges. Clear the full
  // backing store explicitly. The fill/draw operations below still use
  // logical W/H because the context's transform handles the DPR scaling.
  ctx.clearRect(0, 0, ctx.canvas.width, ctx.canvas.height);
  // Background gradient — subtle space backdrop.
  const bg = ctx.createLinearGradient(0, 0, 0, H);
  bg.addColorStop(0, "rgba(38, 32, 60, 0.55)");
  bg.addColorStop(1, "rgba(18, 20, 32, 0.75)");
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, W, H);

  // Hyperplane axis: vertical line through the canvas center, tilted by
  // planeAngle. Negative side = left, Positive side = right.
  const cx = W / 2;
  const cy = H / 2;
  const len = Math.max(W, H);
  const dx = Math.sin(planeAngle) * len;
  const dy = Math.cos(planeAngle) * len;
  // Fill positive / negative half-planes.
  ctx.save();
  ctx.translate(cx, cy);
  ctx.rotate(planeAngle);
  // Positive half = right of the line.
  ctx.fillStyle = "rgba(48, 209, 88, 0.06)";
  ctx.fillRect(0, -H, W, H * 2);
  ctx.fillStyle = "rgba(255, 69, 58, 0.06)";
  ctx.fillRect(-W, -H, W, H * 2);
  ctx.restore();

  // The hyperplane itself.
  ctx.strokeStyle = "oklch(78% 0.18 285)";
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.moveTo(cx - dx, cy - dy);
  ctx.lineTo(cx + dx, cy + dy);
  ctx.stroke();

  // Labels.
  ctx.font = "11px -apple-system, system-ui, sans-serif";
  ctx.fillStyle = "rgba(255, 255, 255, 0.62)";
  ctx.textAlign = "right";
  ctx.fillText("− negative side", W - 14, H - 14);
  ctx.textAlign = "left";
  ctx.fillStyle = "rgba(180, 240, 200, 0.72)";
  ctx.fillText("+ positive side", 14, H - 14);

  // Anchors.
  drawAnchorChips(ctx, W, H, "negative", state.mix.negative, planeAngle);
  drawAnchorChips(ctx, W, H, "positive", state.mix.positive, planeAngle);

  // Result particles — light starbursts emitted from the positive side.
  const parts = state.hyperplane.particles || [];
  for (const p of parts) {
    const t = REDUCED_MOTION.matches
      ? 1
      : Math.min(1, (performance.now() - p.spawn) / 900);
    const ease = 1 - Math.pow(1 - t, 3); // cubic-bezier-ish ease-out
    const x = p.from.x + (p.to.x - p.from.x) * ease;
    const y = p.from.y + (p.to.y - p.from.y) * ease;
    ctx.beginPath();
    const r = 6 + p.score * 8;
    ctx.arc(x, y, r, 0, Math.PI * 2);
    const a = 0.35 + p.score * 0.5;
    ctx.fillStyle = `oklch(78% 0.16 ${130 - p.score * 60} / ${a})`;
    ctx.fill();
    // Score label
    if (t > 0.7) {
      ctx.fillStyle = "rgba(255, 255, 255, 0.78)";
      ctx.font = "10px var(--mono, ui-monospace, monospace)";
      ctx.textAlign = "center";
      ctx.fillText(p.label.slice(0, 12), x, y - r - 4);
    }
  }
}

function drawAnchorChips(ctx, W, H, side, ids, planeAngle) {
  const cx = W / 2;
  const cy = H / 2;
  const radius = Math.min(W, H) * 0.34;
  const horizontalSign = side === "positive" ? 1 : -1;
  ids.forEach((id, idx) => {
    const angleSpread = (idx - (ids.length - 1) / 2) * 0.18;
    const phi = planeAngle + angleSpread + Math.PI / 2 * 0;
    const rx = cx + horizontalSign * radius * Math.cos(angleSpread);
    const ry = cy + radius * Math.sin(angleSpread) * 0.6;
    ctx.beginPath();
    ctx.arc(rx, ry, 11, 0, Math.PI * 2);
    ctx.fillStyle =
      side === "positive"
        ? "oklch(78% 0.16 150)"
        : "oklch(72% 0.20 25)";
    ctx.fill();
    ctx.strokeStyle = "rgba(255,255,255,0.65)";
    ctx.lineWidth = 1.5;
    ctx.stroke();
    ctx.fillStyle = "rgba(0,0,0,0.85)";
    ctx.font = "10px var(--mono, ui-monospace, monospace)";
    ctx.textAlign = "center";
    ctx.fillText(id.slice(0, 4), rx, ry + 3);
  });
}

// Build flying-particle records for the canvas from a fresh result set.
// Maximum N=20 to keep frame work bounded.
function particlesFor(hits) {
  const canvas = document.getElementById("hyperplane-canvas");
  if (!canvas) return [];
  const W = canvas.clientWidth || canvas.width;
  const H = canvas.clientHeight || canvas.height;
  const cx = W / 2;
  const cy = H / 2;
  const out = [];
  hits.slice(0, 20).forEach((h, i) => {
    const a = (i / Math.max(1, hits.length - 1)) * Math.PI - Math.PI / 2;
    const tx = cx + Math.cos(a) * (W * 0.36) + 80;
    const ty = cy + Math.sin(a) * (H * 0.32);
    out.push({
      from: { x: cx, y: cy },
      to: { x: tx, y: ty },
      score: Math.max(0, Math.min(1, h.score ?? 0)),
      label: h.project_name || h.session_id.slice(0, 6),
      spawn: performance.now() + i * 50,
    });
  });
  return out;
}

function paintHyperplaneResults(hits) {
  state.hyperplane.particles = particlesFor(hits);
}

// Stop the hyperplane loop when the mix modal closes (avoid leaking rAFs).
document.addEventListener(
  "DOMContentLoaded",
  () => {
    const m = document.getElementById("mix-modal");
    if (!m) return;
    m.addEventListener("close", () => {
      if (state.hyperplane.raf) {
        cancelAnimationFrame(state.hyperplane.raf);
        state.hyperplane.raf = null;
      }
    });
  },
  { once: true },
);

// ---------------------------------------------------------------------------
// Phase 5 — Replay engine
// ---------------------------------------------------------------------------

function attachReplayEvents() {
  document.getElementById("replay-prev").addEventListener("click", () => stepReplay(-1));
  document.getElementById("replay-next").addEventListener("click", () => stepReplay(+1));
  document.getElementById("replay-play").addEventListener("click", toggleReplayPlay);
  document
    .getElementById("replay-speed")
    .addEventListener("change", (e) => {
      state.replay.speedMs = parseInt(e.target.value, 10);
      if (state.replay.playing) {
        stopReplayTimer();
        startReplayTimer();
      }
    });
  document.getElementById("replay-modal").addEventListener("close", () => {
    stopReplayTimer();
    state.replay.playing = false;
  });
}

async function openReplay(sessionId, hit) {
  const modal = document.getElementById("replay-modal");
  modal.showModal();
  document.getElementById("replay-title").textContent = `Replay — ${hit?.project_name || sessionId.slice(0, 8)}`;
  document.getElementById("replay-detail").innerHTML =
    `<div class="empty">Loading session…</div>`;
  document.getElementById("replay-list").innerHTML = "";
  state.replay = {
    sessionId,
    turns: [],
    cursor: 0,
    playing: false,
    speedMs: parseInt(document.getElementById("replay-speed").value, 10),
    timer: null,
  };
  try {
    const session = await invoke("get_session_turns", { sessionId });
    state.replay.turns = session.turns || [];
    if (!state.replay.turns.length) {
      document.getElementById("replay-detail").innerHTML =
        `<div class="empty">Session has no turns to replay.</div>`;
      return;
    }
    renderReplayList(session);
    renderReplayTurn(0);
  } catch (err) {
    document.getElementById("replay-detail").innerHTML =
      `<div class="empty">Failed to load: ${escapeHtml(String(err))}</div>`;
  }
}

function renderReplayList(session) {
  const list = document.getElementById("replay-list");
  list.innerHTML = "";
  state.replay.turns.forEach((turn, i) => {
    const row = document.createElement("button");
    row.type = "button";
    row.className = "replay-row";
    row.dataset.index = i;
    const role = turn.role === "user" ? "U" : turn.role === "assistant" ? "A" : "S";
    const preview = (turn.text || "")
      .replace(/\s+/g, " ")
      .slice(0, 60);
    const tools = (turn.tool_calls || [])
      .map((t) => t.name)
      .join(", ");
    row.innerHTML = `
      <span class="replay-row-role role-${turn.role}">${role}</span>
      <span class="replay-row-preview">${escapeHtml(preview || tools || "(empty)")}</span>
      <span class="replay-row-meta">${turn.tool_calls?.length ? "🔧" + turn.tool_calls.length : ""}</span>
    `;
    row.addEventListener("click", () => {
      state.replay.cursor = i;
      renderReplayTurn(i);
      stopReplayTimer();
      state.replay.playing = false;
      updatePlayButton();
    });
    list.appendChild(row);
  });
  updateProgress();
}

function renderReplayTurn(i) {
  const turn = state.replay.turns[i];
  if (!turn) return;
  const detail = document.getElementById("replay-detail");
  const ts = turn.timestamp ? new Date(turn.timestamp).toISOString().slice(0, 19).replace("T", " ") : "";

  const toolViz = (turn.tool_calls || [])
    .map((tc) => renderToolCall(tc, turn.tool_results || []))
    .join("");

  const toolResults = (turn.tool_results || [])
    .filter((r) => !(turn.tool_calls || []).some((tc) => tc.id === r.tool_use_id))
    .map(renderStrayResult)
    .join("");

  detail.innerHTML = `
    <div class="turn-meta">
      <span class="turn-role role-${turn.role}">${turn.role}</span>
      <span class="muted">${escapeHtml(ts)}</span>
      ${turn.is_sidechain ? '<span class="badge">sidechain</span>' : ""}
    </div>
    ${turn.text ? `<pre class="turn-text">${escapeHtml(turn.text)}</pre>` : ""}
    ${toolViz}
    ${toolResults}
  `;

  // Highlight the selected row + scroll into view.
  for (const r of document.querySelectorAll(".replay-row")) {
    r.classList.toggle("selected", parseInt(r.dataset.index, 10) === i);
  }
  const selectedRow = document.querySelector(`.replay-row[data-index="${i}"]`);
  if (selectedRow) selectedRow.scrollIntoView({ block: "nearest" });
  state.replay.cursor = i;
  updateProgress();
}

function renderToolCall(tc, results) {
  const result = results.find((r) => r.tool_use_id === tc.id);
  const input = tc.input || {};
  // B4: tool name comes from the model + jsonl source. For Claude Code it's
  // a fixed whitelist, but a foreign jsonl could carry arbitrary HTML — be
  // consistent and escape it everywhere.
  const name = escapeHtml(tc.name || "?");
  // Inline `tool-error` class up front so we don't rely on String.replace.
  const errCls = result && result.is_error ? " tool-error" : "";
  let body = "";
  switch (tc.name) {
    case "Bash": {
      body = `
        <div class="tool-block tool-bash${errCls}">
          <div class="tool-head">Bash · ${escapeHtml(input.description || "")}</div>
          <pre class="terminal">$ ${escapeHtml(input.command || "")}</pre>
          ${result ? `<pre class="terminal output">${escapeHtml(result.content.slice(0, 1200))}</pre>` : ""}
        </div>`;
      break;
    }
    case "Edit":
    case "MultiEdit": {
      body = `
        <div class="tool-block tool-edit${errCls}">
          <div class="tool-head">Edit · <code>${escapeHtml(input.file_path || "")}</code></div>
          ${input.old_string ? `<pre class="diff diff-old">- ${escapeHtml(input.old_string.slice(0, 600))}</pre>` : ""}
          ${input.new_string ? `<pre class="diff diff-new">+ ${escapeHtml(input.new_string.slice(0, 600))}</pre>` : ""}
        </div>`;
      break;
    }
    case "Write": {
      body = `
        <div class="tool-block tool-edit${errCls}">
          <div class="tool-head">Write · <code>${escapeHtml(input.file_path || "")}</code></div>
          <pre class="diff diff-new">${escapeHtml(String(input.content || "").slice(0, 800))}</pre>
        </div>`;
      break;
    }
    case "Read": {
      body = `
        <div class="tool-block tool-read${errCls}">
          <div class="tool-head">Read · <code>${escapeHtml(input.file_path || "")}</code></div>
          ${result ? `<pre class="terminal output">${escapeHtml(result.content.slice(0, 800))}</pre>` : ""}
        </div>`;
      break;
    }
    case "WebFetch":
    case "WebSearch": {
      body = `
        <div class="tool-block${errCls}">
          <div class="tool-head">${name} · ${escapeHtml(input.url || input.query || "")}</div>
          ${result ? `<pre class="terminal output">${escapeHtml(result.content.slice(0, 600))}</pre>` : ""}
        </div>`;
      break;
    }
    case "Task":
    case "Agent": {
      body = `
        <div class="tool-block${errCls}">
          <div class="tool-head">${name} · ${escapeHtml(input.subagent_type || input.description || "")}</div>
          <pre class="diff diff-new">${escapeHtml((input.prompt || "").slice(0, 600))}</pre>
        </div>`;
      break;
    }
    default: {
      body = `
        <div class="tool-block${errCls}">
          <div class="tool-head">${name}</div>
          <pre class="terminal">${escapeHtml(JSON.stringify(input).slice(0, 600))}</pre>
          ${result ? `<pre class="terminal output">${escapeHtml(result.content.slice(0, 600))}</pre>` : ""}
        </div>`;
    }
  }
  return body;
}

function renderStrayResult(r) {
  return `
    <div class="tool-block ${r.is_error ? "tool-error" : ""}">
      <div class="tool-head">tool_result</div>
      <pre class="terminal output">${escapeHtml(r.content.slice(0, 800))}</pre>
    </div>`;
}

function stepReplay(delta) {
  const total = state.replay.turns.length;
  if (!total) return;
  const next = Math.max(0, Math.min(total - 1, state.replay.cursor + delta));
  renderReplayTurn(next);
}

function toggleReplayPlay() {
  state.replay.playing = !state.replay.playing;
  updatePlayButton();
  if (state.replay.playing) startReplayTimer();
  else stopReplayTimer();
}

function startReplayTimer() {
  stopReplayTimer();
  state.replay.timer = setInterval(() => {
    const next = state.replay.cursor + 1;
    if (next >= state.replay.turns.length) {
      stopReplayTimer();
      state.replay.playing = false;
      updatePlayButton();
      return;
    }
    renderReplayTurn(next);
  }, state.replay.speedMs);
}

function stopReplayTimer() {
  if (state.replay.timer) {
    clearInterval(state.replay.timer);
    state.replay.timer = null;
  }
}

function updatePlayButton() {
  document.getElementById("replay-play").textContent = state.replay.playing ? "❚❚" : "▶";
}

function updateProgress() {
  document.getElementById("replay-progress").textContent =
    `${state.replay.cursor + 1} / ${state.replay.turns.length}`;
}

// ---------------------------------------------------------------------------
// Phase 6 — Proactive recall (polling)
// ---------------------------------------------------------------------------

const RECALL_POLL_MS = 12_000;

async function startRecallPolling() {
  try {
    await pollRecall();
  } catch {}
  setInterval(pollRecall, RECALL_POLL_MS);
}

async function pollRecall() {
  try {
    const recent = await invoke("tail_recent_errors", { sinceSeconds: 90 });
    if (!recent || !recent.length) return;
    // Pick the most-recent error not already shown.
    for (const ev of recent) {
      const key = `${ev.session_id}::${ev.error_text.slice(0, 80)}`;
      if (state.recall.dismissedKeys.has(key)) continue;
      const hits = await recallCached(ev.error_text, 3);
      // Filter out the current (still-failing) session itself.
      const useful = hits.filter((h) => h.session_id !== ev.session_id);
      if (!useful.length) continue;
      showRecallBanner(ev, useful);
      state.recall.lastBannerError = { ev, key, hits: useful };
      return; // only one banner at a time
    }
  } catch (err) {
    // Silent — Qdrant may not be ready yet.
  }
}

// P2: deduplicate recall calls by error_text within a short TTL so the
// 12 s polling loop doesn't re-embed the same error over and over.
async function recallCached(errorText, limit) {
  const cache = state.recall.cache;
  const prev = cache.get(errorText);
  if (prev && Date.now() - prev.ts < RECALL_CACHE_TTL_MS) {
    return prev.hits;
  }
  const hits = await invoke("recall", { errorText, limit });
  cache.set(errorText, { hits, ts: Date.now() });
  if (cache.size > RECALL_CACHE_MAX) {
    const oldest = cache.keys().next().value;
    cache.delete(oldest);
  }
  return hits;
}

function showRecallBanner(ev, hits) {
  const banner = document.getElementById("recall-banner");
  const detail = document.getElementById("recall-banner-detail");
  detail.textContent = `${ev.project_name || "?"} just hit: "${ev.error_text.slice(0, 120).replace(/\s+/g, " ")}" — ${hits.length} past session(s) may help`;
  banner.classList.remove("hidden");
}

function attachRecallBannerEvents() {
  document.getElementById("recall-banner-dismiss").addEventListener("click", () => {
    if (state.recall.lastBannerError) {
      state.recall.dismissedKeys.add(state.recall.lastBannerError.key);
    }
    document.getElementById("recall-banner").classList.add("hidden");
  });
  document.getElementById("recall-banner-open").addEventListener("click", () => {
    const ctx = state.recall.lastBannerError;
    if (!ctx) return;
    document.getElementById("recall-banner").classList.add("hidden");
    state.recall.dismissedKeys.add(ctx.key);
    // Open replay for the first past-fix candidate.
    const target = ctx.hits[0];
    openReplay(target.session_id, target);
  });
}
