// Memex frontend — vanilla JS shell wiring 5 Qdrant-backed commands.
// Tauri's `withGlobalTauri: true` puts the IPC bridge on window.__TAURI__,
// so we can stay plain ESM without a build step.

const { invoke } = window.__TAURI__.core;

const LENSES = ["content", "tool", "path", "error", "code"];

const state = {
  query: "",
  weights: Object.fromEntries(LENSES.map((k) => [k, 1.0])),
  hits: [],
  selected: null,
  mix: { positive: [], negative: [] },
  collectionPoints: 0,
  // B3: monotonically-increasing query id; renderResults drops responses
  // whose generation is older than the latest dispatched query.
  queryGen: 0,
  // Time Machine stack: loaded on boot via list_sessions (no Qdrant needed),
  // shown when there's no active search query.
  stack: [],
  stackFocus: 0,
  mode: "stack", // "stack" | "search"
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
  // Kick off both pollers; the stack uses pure jsonl parsing so it succeeds
  // even before Qdrant comes up, giving the user something to look at
  // immediately.
  loadInitialStack();
  await pollUntilReady();
  startRecallPolling();
});

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
  try {
    const hits = await invoke("lens_search", {
      query: state.query,
      weights: state.weights,
      limit: 20,
    });
    // B3: if a newer query has already been dispatched, drop this stale
    // response so we don't overwrite a fresher result list.
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

function buildLensSliders() {
  const root = document.getElementById("lens-sliders");
  root.innerHTML = "";
  for (const name of LENSES) {
    const wrap = document.createElement("div");
    wrap.className = "slider";
    wrap.innerHTML = `
      <div class="slider-label">
        <span>${name}</span>
        <span class="slider-value" data-for="${name}">1.00</span>
      </div>
      <input type="range" min="0" max="2" step="0.05" value="1.0" data-name="${name}" />
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
  for (const name of LENSES) state.weights[name] = 1.0;
  document
    .querySelectorAll("#lens-sliders input")
    .forEach((i) => (i.value = "1.0"));
  document
    .querySelectorAll(".slider-value")
    .forEach((s) => (s.textContent = "1.00"));
  if (state.query) runLensSearch();
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
    const vecBreak = Object.entries(h.vector_scores || {})
      .sort((a, b) => b[1] - a[1])
      .map(([k, v]) => `<span class="vec-chip">${k} ${v.toFixed(2)}</span>`)
      .join("");
    card.innerHTML = `
      <header>
        <span class="score">${(h.score ?? 0).toFixed(3)}</span>
        <span class="proj">${escapeHtml(h.project_name || "?")}</span>
        <span class="ts">${ts}</span>
      </header>
      <h3 class="title">${escapeHtml(title)}</h3>
      <div class="vec-breakdown">${vecBreak}</div>
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
        return;
      }
    }
    renderInspector(payload, sessionId);
    // Path 2 — auto-load predictive next actions for the selected session.
    // Non-blocking; the panel populates when the backend returns.
    if (!silent) loadPredictions(sessionId);
  } catch (err) {
    if (silent) return;
    inspector.innerHTML = `<div class="empty">Error: ${escapeHtml(String(err))}</div>`;
  }
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
  // Always render INSIDE the inspector, below the payload kvs section.
  const inspector = document.getElementById("inspector");
  let panel = document.getElementById("prediction-panel");
  if (!panel) {
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
    <div class="prediction-list"></div>
  `;
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
  inspector.innerHTML = `
    <header class="inspector-head">
      <h3>${escapeHtml(payload.project_name || "session")}</h3>
      <code>${escapeHtml(sessionId)}</code>
    </header>
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
  canvas.innerHTML = `<div class="empty">Computing MST…</div>`;
  try {
    const topo = await invoke("topology", { sample: 80, perPoint: 6 });
    renderTopology3D(topo, canvas);
  } catch (err) {
    canvas.innerHTML = `<div class="empty">Topology failed: ${escapeHtml(String(err))}</div>`;
  }
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
  mount.innerHTML = "";
  const { nodes, edges } = topo;
  const statsEl = document.getElementById("topology-stats");
  const legendEl = document.getElementById("topology-legend");
  if (statsEl) statsEl.innerHTML = "";
  if (legendEl) legendEl.innerHTML = "";

  if (!nodes.length) {
    mount.innerHTML = `<div class="empty">No nodes yet — re-index first.</div>`;
    return;
  }
  if (typeof window.ForceGraph3D !== "function") {
    mount.innerHTML = `<div class="empty">3D engine failed to load (vendor/3d-force-graph.min.js).</div>`;
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
    // Cross-project bridges in white = "look here, an idea jumped between
    // projects". In-project edges in muted blue = the within-cluster glue.
    .linkColor((l) =>
      l.cross
        ? `rgba(255, 214, 10, ${0.55 + l.similarity * 0.35})`
        : `rgba(10, 132, 255, ${0.30 + l.similarity * 0.45})`,
    )
    .linkOpacity(1)
    .linkWidth((l) => (l.cross ? 1.4 + l.similarity * 2.5 : 0.4 + l.similarity * 1.6))
    .linkDirectionalParticles((l) => (l.cross ? 2 : 0))
    .linkDirectionalParticleWidth(1.2)
    .linkDirectionalParticleColor(() => "#ffd60a")
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
  document.getElementById("mix-modal").showModal();
}

function addToMix(side, sessionId) {
  if (!state.mix[side].includes(sessionId)) {
    state.mix[side].push(sessionId);
  }
  renderMixDropzones();
}

function removeFromMix(side, sessionId) {
  state.mix[side] = state.mix[side].filter((s) => s !== sessionId);
  renderMixDropzones();
}

function renderMixDropzones() {
  for (const side of ["positive", "negative"]) {
    const root = document.getElementById(`mix-${side}`);
    root.innerHTML = "";
    if (!state.mix[side].length) {
      const hint = document.createElement("span");
      hint.className = "dropzone-hint";
      hint.textContent = "click + pos / − neg on a card to add…";
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
  try {
    const hits = await invoke("mix_match", {
      positive: state.mix.positive,
      negative: state.mix.negative,
      limit: 10,
    });
    if (!hits.length) {
      out.textContent = "No discovery hits.";
      return;
    }
    out.innerHTML = "<h4>Discovered</h4>";
    for (const h of hits) {
      const row = document.createElement("div");
      row.className = "mix-row";
      row.textContent = `${h.score.toFixed(3)}  ${h.project_name}  ${h.session_id}`;
      out.appendChild(row);
    }
  } catch (err) {
    out.textContent = `Mix failed: ${err}`;
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
