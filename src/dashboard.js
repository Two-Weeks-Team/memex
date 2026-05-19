// Memex Dashboard — wired to live Tauri commands.
//
// Data sources (all existing, no new Rust commands required):
//   - list_sessions       → KPI / heatmap / project bars / week diff / today brief
//   - tail_recent_errors  → recall queue (recent error scan)
//   - recall              → cross-session hits for each fresh error
//   - predict_next_actions → today's brief next-action prediction
//
// Widgets where the existing Rust commands don't give us enough granularity
// (per-tool name counts, error-text clustering, tool co-occurrence) fall
// back to a clearly-labelled placeholder until we add a dedicated
// `dashboard_aggregates` command.

const { invoke } = window.__TAURI__.core;

const PROJECT_COLORS = [
  "#5aa6ff", "#74e0c1", "#ff8a65", "#9d7bff",
  "#ffb86b", "#ff6b8a", "#6bd96b", "#7ad6ff",
  "#c39bff", "#ffd166",
];

const state = {
  rangeDays: 90,   // 7 / 30 / 90 / null (all-time)
  sessions: [],    // raw SessionSummary[]
  history: null,   // PromptHistoryStats — base layer from history.jsonl
};

// --------------------------------------------------------------------------
// Boot
// --------------------------------------------------------------------------

document.addEventListener("DOMContentLoaded", async () => {
  attachRangeChips();
  document.getElementById("dashAsOf").textContent =
    `as of ${new Date().toLocaleDateString("en-CA")} ${new Date().toLocaleTimeString("en-GB", {hour:"2-digit",minute:"2-digit"})}`;

  // Always render the static placeholders first so the page never looks empty.
  renderToolParetoPlaceholder();
  renderErrorCatalogPlaceholder();
  renderSkillGraph();
  renderWeekStatsLoading();

  // Fire list_sessions + prompt_history_stats in parallel — they don't
  // depend on each other and history can be slow (24k+ entries).
  const [sessionsPromise, historyPromise] = [
    invoke("list_sessions", { limit: 2000 }),
    invoke("prompt_history_stats").catch((e) => {
      console.warn("prompt_history_stats failed:", e);
      return null;
    }),
  ];

  try {
    const sessions = await sessionsPromise;
    state.sessions = sessions || [];
  } catch (err) {
    console.warn("list_sessions failed:", err);
    showFatalLoad(String(err));
  }

  try {
    const history = await historyPromise;
    state.history = history || null;
  } catch (e) {
    state.history = null;
  }

  renderAll();
  // Recall queue + today's brief depend on per-session detail / IPC chains —
  // do them after first paint.
  loadRecallQueue();
  loadTodayBrief();
});

function attachRangeChips() {
  document.querySelectorAll(".dash-top .chip-tab").forEach((c) => {
    c.addEventListener("click", () => {
      document.querySelectorAll(".dash-top .chip-tab").forEach((x) => x.classList.remove("active"));
      c.classList.add("active");
      const r = c.dataset.range;
      state.rangeDays = r === "all" ? null : parseInt(r, 10);
      renderAll();
    });
  });
}

function showFatalLoad(msg) {
  document.getElementById("kpiSessionsDelta").textContent = `error: ${msg.slice(0, 60)}`;
  document.getElementById("kpiSessionsDelta").classList.add("down");
}

// --------------------------------------------------------------------------
// Filtering + render orchestration
// --------------------------------------------------------------------------

function rangedSessions() {
  if (!state.rangeDays) return state.sessions;
  const cutoff = Date.now() - state.rangeDays * 24 * 60 * 60 * 1000;
  return state.sessions.filter((s) => {
    const t = parseIso(s.start_iso);
    return t ? t.getTime() >= cutoff : false;
  });
}

function parseIso(s) {
  if (!s) return null;
  const d = new Date(s);
  return isNaN(d.getTime()) ? null : d;
}

function renderAll() {
  const ranged = rangedSessions();
  renderKpi(ranged);
  renderArchaeoCard();
  renderHeatmap();     // heatmap span is governed by history.jsonl extent
  renderProjectBars(ranged);
  renderWeekStats();
}

// ---------------------------------------------------------------------------
// Data archaeology card — "what Memex preserves against Anthropic's silent
// migrations". This is the new top-of-dashboard value proposition.
// ---------------------------------------------------------------------------

function renderArchaeoCard() {
  const wrap = document.getElementById("archaeoBody");
  const h = state.history;
  const sessions = state.sessions;

  // Counts:
  // - legacy sessions: project_name === "(legacy transcript)"
  // - modern sessions: everything else (incl. blank project_name)
  const legacyCount = sessions.filter((s) => s.project_name === "(legacy transcript)").length;
  const modernCount = sessions.length - legacyCount;

  const promptsTotal = h?.total_prompts || 0;
  const earliest = h?.earliest_ms ? new Date(h.earliest_ms) : null;
  const latest = h?.latest_ms ? new Date(h.latest_ms) : null;
  const corpusDays = earliest && latest
    ? Math.max(1, Math.ceil((latest.getTime() - earliest.getTime()) / 86_400_000))
    : 0;

  wrap.innerHTML = `
    <div class="archaeo-grid">
      <div class="archaeo-stat">
        <div class="lbl">Indexed sessions</div>
        <div class="val">${sessions.length.toLocaleString()}</div>
        <div class="sub">${modernCount} modern · ${legacyCount} legacy transcript${legacyCount===1?"":"s"}</div>
      </div>
      <div class="archaeo-stat">
        <div class="lbl">Prompts preserved</div>
        <div class="val">${promptsTotal.toLocaleString()}</div>
        <div class="sub">from ~/.claude/history.jsonl</div>
      </div>
      <div class="archaeo-stat">
        <div class="lbl">Corpus span</div>
        <div class="val">${corpusDays} d</div>
        <div class="sub">${earliest ? "since " + earliest.toLocaleDateString("en-CA") : "no history yet"}</div>
      </div>
      <div class="archaeo-stat">
        <div class="lbl">Distinct projects</div>
        <div class="val">${h?.project_count?.toLocaleString() ?? "—"}</div>
        <div class="sub">across the full timeline</div>
      </div>
    </div>
    <div class="archaeo-note">
      <strong>왜 이 카드가 있나:</strong>
      Claude Code는 v2.1.114 즈음 세션 저장 경로를 <code class="mono">~/.claude/transcripts/</code>에서
      <code class="mono">~/.claude/projects/</code>로 silent migration했고,
      자동 업데이트가 옛 .jsonl을 청소한 사례가
      <a href="https://github.com/anthropics/claude-code/issues/41591" target="_blank" rel="noopener">#41591</a> ·
      <a href="https://github.com/anthropics/claude-code/issues/54907" target="_blank" rel="noopener">#54907</a> ·
      <a href="https://github.com/anthropics/claude-code/issues/48782" target="_blank" rel="noopener">#48782</a>
      등 수십 건의 OPEN GitHub issue로 보고됐습니다.
      Memex는 두 경로 모두 indexing하고 Qdrant snapshot으로 영구 보존 — Anthropic이 잊어도 Memex는 기억합니다.
    </div>
    <div class="archaeo-actions">
      <button class="btn-mini" id="archaeoSnapshot">📦 Export snapshot now</button>
      <span class="muted" style="font-size:11px;" id="archaeoSnapshotStatus"></span>
    </div>
  `;

  const btn = document.getElementById("archaeoSnapshot");
  if (btn) {
    btn.addEventListener("click", async () => {
      const statusEl = document.getElementById("archaeoSnapshotStatus");
      btn.disabled = true;
      statusEl.textContent = "exporting…";
      try {
        const result = await invoke("snapshot_export_default");
        statusEl.textContent = `✓ ${result}`;
        statusEl.style.color = "rgb(150,240,210)";
      } catch (err) {
        statusEl.textContent = `failed: ${String(err).slice(0, 80)}`;
        statusEl.style.color = "#ff6b8a";
      } finally {
        btn.disabled = false;
      }
    });
  }
}

// --------------------------------------------------------------------------
// KPI tiles
// --------------------------------------------------------------------------

function renderKpi(sessions) {
  const total = sessions.length;
  const turns = sessions.reduce((acc, s) => acc + (s.user_turns || 0) + (s.assistant_turns || 0), 0);
  const tools = sessions.reduce((acc, s) => acc + (s.tool_count || 0), 0);

  const activeDates = new Set();
  for (const s of sessions) {
    const d = parseIso(s.start_iso);
    if (d) activeDates.add(d.toLocaleDateString("en-CA"));
  }

  document.getElementById("kpiSessions").textContent = total.toLocaleString();
  document.getElementById("kpiTurns").textContent = turns.toLocaleString();
  document.getElementById("kpiTools").textContent = tools.toLocaleString();
  document.getElementById("kpiActive").textContent =
    `${activeDates.size} ${state.rangeDays ? `/ ${state.rangeDays}` : "(total)"}`;

  // Streak — count consecutive days backwards from today that exist in activeDates.
  let streak = 0;
  const cursor = new Date();
  cursor.setHours(0, 0, 0, 0);
  for (;;) {
    if (activeDates.has(cursor.toLocaleDateString("en-CA"))) {
      streak++;
      cursor.setDate(cursor.getDate() - 1);
    } else {
      break;
    }
  }
  document.getElementById("kpiStreak").textContent =
    streak > 0 ? `streak ${streak} d` : "no streak today";
  document.getElementById("kpiStreak").className =
    streak > 0 ? "delta up" : "delta flat";

  // Delta vs the prior equal-length window.
  if (state.rangeDays) {
    const cutoffPrev = Date.now() - 2 * state.rangeDays * 86_400_000;
    const cutoffCur = Date.now() - state.rangeDays * 86_400_000;
    const prev = state.sessions.filter((s) => {
      const t = parseIso(s.start_iso);
      return t && t.getTime() >= cutoffPrev && t.getTime() < cutoffCur;
    });
    setDelta("kpiSessionsDelta", total, prev.length, "vs prev");
    setDelta(
      "kpiTurnsDelta",
      turns,
      prev.reduce((a, s) => a + (s.user_turns || 0) + (s.assistant_turns || 0), 0),
      "vs prev",
    );
    setDelta(
      "kpiToolsDelta",
      tools,
      prev.reduce((a, s) => a + (s.tool_count || 0), 0),
      "vs prev",
    );
  } else {
    setText("kpiSessionsDelta", "all-time", "flat");
    setText("kpiTurnsDelta", "all-time", "flat");
    setText("kpiToolsDelta", "all-time", "flat");
  }
}

function setDelta(id, cur, prev, suffix) {
  const el = document.getElementById(id);
  if (prev === 0) {
    el.textContent = `${suffix}: —`;
    el.className = "delta flat";
    return;
  }
  const diff = cur - prev;
  const pct = Math.round((diff / Math.max(prev, 1)) * 100);
  const arrow = diff > 0 ? "↑" : diff < 0 ? "↓" : "↔";
  const sign = diff > 0 ? "+" : "";
  el.textContent = `${arrow} ${sign}${diff.toLocaleString()} (${pct >= 0 ? "+" : ""}${pct}%) ${suffix}`;
  el.className = "delta " + (diff > 0 ? "up" : diff < 0 ? "down" : "flat");
}
function setText(id, text, cls) {
  const el = document.getElementById(id);
  el.textContent = text;
  el.className = "delta " + (cls || "flat");
}

// --------------------------------------------------------------------------
// Activity heatmap — two-layer:
//   • BASE (blue): per-day prompt counts from ~/.claude/history.jsonl
//     (typically 6–12 months — survives the silent session-jsonl cleanup)
//   • OVERLAY (green outline): days where Memex has indexed session jsonls
//     (typically 30 d in projects/ + however much survived in transcripts/)
// The grid auto-fits the *history.jsonl* span, not the session span, so the
// timeline reflects the user's full working history.
// --------------------------------------------------------------------------

function renderHeatmap() {
  const grid = document.getElementById("heatmap");
  grid.innerHTML = "";
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const DAY = 86_400_000;

  // --- 1. Build session buckets (overlay) ---
  const sessionBuckets = new Map();
  let sessionEarliest = null;
  for (const s of state.sessions) {
    const d = parseIso(s.start_iso);
    if (!d) continue;
    const ds = new Date(d); ds.setHours(0, 0, 0, 0);
    if (!sessionEarliest || ds < sessionEarliest) sessionEarliest = ds;
    const key = ds.toLocaleDateString("en-CA");
    const cur = sessionBuckets.get(key) || { sessions: 0, turns: 0 };
    cur.sessions += 1;
    cur.turns += (s.user_turns || 0) + (s.assistant_turns || 0);
    sessionBuckets.set(key, cur);
  }

  // --- 2. history.jsonl prompt buckets (base layer) ---
  // Server returns { by_day: { "YYYY-MM-DD": count, ... }, earliest_ms, latest_ms }
  const historyByDay = (state.history && state.history.by_day) || {};
  const promptCounts = Object.values(historyByDay).filter((x) => x > 0).sort((a, b) => a - b);
  function promptLevel(n) {
    if (n <= 0) return 0;
    if (promptCounts.length === 0) return 1;
    const q = (p) => promptCounts[Math.min(promptCounts.length - 1, Math.floor(promptCounts.length * p))];
    if (n <= q(0.25)) return 1;
    if (n <= q(0.55)) return 2;
    if (n <= q(0.85)) return 3;
    return 4;
  }

  // --- 3. Choose grid start: prefer history.earliest, fall back to session
  //        earliest, finally last 30 d. Cap at 365 d for visual density. ---
  let earliest = null;
  if (state.history?.earliest_ms) {
    earliest = new Date(state.history.earliest_ms);
    earliest.setHours(0, 0, 0, 0);
  } else if (sessionEarliest) {
    earliest = sessionEarliest;
  }

  let gridStart;
  if (earliest) {
    const padded = new Date(earliest.getTime() - 7 * DAY);
    const cap = new Date(today.getTime() - 365 * DAY);
    gridStart = padded < cap ? cap : padded;
  } else {
    gridStart = new Date(today.getTime() - 30 * DAY);
  }
  while (gridStart.getDay() !== 1) gridStart.setDate(gridStart.getDate() - 1);

  const daysSpan = Math.ceil((today.getTime() - gridStart.getTime()) / DAY) + 7;
  const cursor = new Date(gridStart);

  const tip = document.getElementById("heatTip");
  let totalSessions = 0;
  let totalPrompts = 0;
  let coveredDays = 0;
  for (let i = 0; i < daysSpan; i++) {
    const cell = document.createElement("div");
    cell.className = "hcell";
    if (cursor > today) {
      cell.style.opacity = ".15";
    } else {
      const key = cursor.toLocaleDateString("en-CA");
      const prompts = historyByDay[key] || 0;
      const sb = sessionBuckets.get(key);
      const lvl = promptLevel(prompts);
      if (lvl > 0) cell.classList.add("b" + lvl);
      if (sb) cell.classList.add("has-session");
      if (prompts > 0) coveredDays += 1;
      totalPrompts += prompts;
      if (sb) totalSessions += sb.sessions;

      const dateStr = key;
      cell.addEventListener("mouseenter", () => {
        const parts = [`${dateStr}`];
        if (prompts > 0) parts.push(`${prompts} prompt${prompts === 1 ? "" : "s"}`);
        if (sb) parts.push(`${sb.sessions} session${sb.sessions === 1 ? "" : "s"} · ${sb.turns} turns`);
        if (parts.length === 1) parts.push("no activity");
        tip.textContent = parts.join(" · ");
        tip.style.display = "block";
      });
      cell.addEventListener("mousemove", (e) => {
        tip.style.left = e.clientX + 12 + "px";
        tip.style.top = e.clientY + 12 + "px";
      });
      cell.addEventListener("mouseleave", () => { tip.style.display = "none"; });
    }
    grid.appendChild(cell);
    cursor.setDate(cursor.getDate() + 1);
  }

  // Footer label — show both layers' coverage so the gap between them is loud.
  const totalEl = document.getElementById("heatTotal");
  if (state.history?.earliest_ms) {
    const startStr = new Date(state.history.earliest_ms).toLocaleDateString("en-CA");
    const spanDays = Math.max(1, Math.ceil((today.getTime() - state.history.earliest_ms) / DAY));
    totalEl.textContent =
      `${totalPrompts.toLocaleString()} prompts over ${spanDays} d (since ${startStr}) · ` +
      `${totalSessions.toLocaleString()} session jsonls indexed`;
  } else if (sessionEarliest) {
    totalEl.textContent = `${totalSessions} sessions · no history.jsonl`;
  } else {
    totalEl.textContent = `no activity`;
  }

  // Title — make the timeline span loud.
  const titleEl = document.querySelector(".heatmap-card .card-head .title");
  if (titleEl) {
    if (state.history?.earliest_ms) {
      const spanDays = Math.max(1, Math.ceil((today.getTime() - state.history.earliest_ms) / DAY));
      titleEl.textContent = `활동 히트맵 — corpus ${spanDays}일 (history.jsonl base · sessions overlay)`;
    } else if (sessionEarliest) {
      const spanDays = Math.max(1, Math.ceil((today.getTime() - sessionEarliest.getTime()) / DAY) + 1);
      titleEl.textContent = `활동 히트맵 — corpus ${spanDays}일`;
    }
  }
}

// --------------------------------------------------------------------------
// Project bars
// --------------------------------------------------------------------------

function renderProjectBars(sessions) {
  const wrap = document.getElementById("projectBars");
  wrap.innerHTML = "";

  const agg = new Map();
  for (const s of sessions) {
    const name = (s.project_name || "(unknown)").trim() || "(unknown)";
    const cur = agg.get(name) || { name, sessions: 0, tools: 0, turns: 0 };
    cur.sessions += 1;
    cur.tools += s.tool_count || 0;
    cur.turns += (s.user_turns || 0) + (s.assistant_turns || 0);
    agg.set(name, cur);
  }
  const rows = [...agg.values()].sort((a, b) => b.sessions - a.sessions);

  if (rows.length === 0) {
    wrap.innerHTML = `<div class="muted">이 범위에 세션이 없습니다.</div>`;
    return;
  }

  // Tail-merge: show top 8 + "N others"
  const head = rows.slice(0, 8);
  const tail = rows.slice(8);
  if (tail.length > 0) {
    head.push({
      name: `${tail.length} other project${tail.length === 1 ? "" : "s"}`,
      sessions: tail.reduce((a, x) => a + x.sessions, 0),
      tools: tail.reduce((a, x) => a + x.tools, 0),
      turns: tail.reduce((a, x) => a + x.turns, 0),
      _tail: true,
    });
  }
  const max = head[0].sessions;

  head.forEach((r, i) => {
    const color = r._tail ? "rgba(255,255,255,.25)" : PROJECT_COLORS[i % PROJECT_COLORS.length];
    const row = document.createElement("div");
    row.className = "proj-row";
    const pct = (r.sessions / max) * 100;
    row.innerHTML = `
      <div class="pname">
        <span class="pd" style="background:${color}"></span>
        <span class="pn-text" title="${escapeHtml(r.name)}">${escapeHtml(r.name)}</span>
      </div>
      <div class="proj-bar"><span style="width:${pct}%; background:${color}; opacity:.85"></span></div>
      <div class="proj-meta">${r.sessions} · ${r.turns.toLocaleString()} t</div>`;
    wrap.appendChild(row);
  });
}

// --------------------------------------------------------------------------
// Tool Pareto — placeholder until per-tool counts come from a Rust agg.
// list_sessions returns total tool_count only, not per-name. Show the top
// 8 sessions by tool usage as a stand-in so the card is not blank.
// --------------------------------------------------------------------------

function renderToolParetoPlaceholder() {
  const wrap = document.getElementById("toolBars");
  wrap.innerHTML = `
    <div class="muted" style="margin-bottom:8px">
      개별 도구 이름 카운트는 새 Tauri command(<code>dashboard_aggregates</code>) 도입 후 채워집니다.
      지금은 세션별 총 도구 호출 수 상위 8개를 표시합니다.
    </div>
    <div id="toolBarsBody"></div>`;
}

function renderToolParetoFromSessions() {
  const wrap = document.getElementById("toolBarsBody");
  if (!wrap) return;
  wrap.innerHTML = "";
  const rows = [...state.sessions]
    .sort((a, b) => (b.tool_count || 0) - (a.tool_count || 0))
    .slice(0, 8);
  const max = rows[0]?.tool_count || 1;
  for (const r of rows) {
    const row = document.createElement("div");
    row.className = "tool-row";
    row.innerHTML = `
      <span class="tname" title="${escapeHtml(r.project_name || "")}">
        ${escapeHtml((r.ai_title || r.project_name || r.session_id || "").slice(0, 18))}
      </span>
      <div class="tool-bar"><span style="width:${((r.tool_count || 0) / max) * 100}%"></span></div>
      <span class="tcount">${(r.tool_count || 0).toLocaleString()}</span>`;
    wrap.appendChild(row);
  }
}

// --------------------------------------------------------------------------
// Error catalog — placeholder. has_errors signal is in payload but text
// extraction lives in the parser. We surface the count of session with
// errors per project as a stand-in.
// --------------------------------------------------------------------------

function renderErrorCatalogPlaceholder() {
  const wrap = document.getElementById("errorList");
  wrap.innerHTML = `
    <div class="muted" style="margin-bottom:8px">
      에러 텍스트 추출/클러스터링은 새 Tauri command 도입 후 채워집니다.
      지금은 has_errors 플래그가 켜진 세션 목록을 최근 순으로 표시합니다.
    </div>
    <div id="errorListBody"></div>`;
}

function renderErrorListFromSessions() {
  const wrap = document.getElementById("errorListBody");
  if (!wrap) return;
  wrap.innerHTML = "";
  const rows = [...state.sessions]
    .filter((s) => s.has_errors)
    .sort((a, b) => (b.start_iso || "").localeCompare(a.start_iso || ""))
    .slice(0, 5);
  if (rows.length === 0) {
    wrap.innerHTML = `<div class="muted">no errors in current range — nice.</div>`;
    return;
  }
  for (const r of rows) {
    const div = document.createElement("div");
    div.className = "err-row";
    const ago = relativeAgo(parseIso(r.start_iso));
    div.innerHTML = `
      <span class="etxt" title="${escapeHtml(r.ai_title || "")}">
        ${escapeHtml((r.ai_title || r.project_name || r.session_id).slice(0, 70))}
      </span>
      <span class="meta">${escapeHtml(r.project_name || "?")} · ${ago}</span>
      <span class="status unsolved">has_errors</span>`;
    wrap.appendChild(div);
  }
}

// --------------------------------------------------------------------------
// Recall queue — tail_recent_errors → for each, recall() and keep
// cross-session hits ≥ 0.65.
// --------------------------------------------------------------------------

async function loadRecallQueue() {
  const wrap = document.getElementById("recallList");
  try {
    const errors = await invoke("tail_recent_errors", { sinceSeconds: 7 * 86_400 });
    if (!errors || errors.length === 0) {
      wrap.innerHTML = `<div class="muted">최근 7 일 내 에러 없음.</div>`;
      return;
    }
    const seen = new Set();
    const items = [];
    for (const ev of errors.slice(0, 8)) {
      const sigKey = ev.session_id + "::" + (ev.error_text || "").slice(0, 80);
      if (seen.has(sigKey)) continue;
      seen.add(sigKey);
      try {
        const hits = await invoke("recall", { errorText: ev.error_text, limit: 4 });
        const cross = (hits || []).filter((h) => h.session_id !== ev.session_id && h.score >= 0.65);
        if (cross.length === 0) continue;
        items.push({ ev, top: cross[0], cross });
        if (items.length >= 4) break;
      } catch (e) {
        console.warn("recall failed for", ev, e);
      }
    }
    if (items.length === 0) {
      wrap.innerHTML = `<div class="muted">recall 매치 없음 — 새 영역의 에러일 가능성.</div>`;
      return;
    }
    wrap.innerHTML = "";
    for (const { ev, top, cross } of items) {
      const ago = relativeAgo(parseIso(ev.seen_at_iso));
      const row = document.createElement("div");
      row.className = "recall-row";
      row.innerHTML = `
        <div class="rhead">
          <span>${escapeHtml(ev.project_name || "(project)")} · ${ago}</span>
          <span>match ${(top.score * 100).toFixed(0)}% in <strong>${escapeHtml(top.project_name || "?")}</strong></span>
        </div>
        <div class="rtxt">${escapeHtml((ev.error_text || "").slice(0, 200))}</div>
        <div class="ractions">
          <button class="btn-mini primary" data-action="open" data-sid="${escapeAttr(top.session_id)}">
            Open replay → ${escapeHtml((top.ai_title || top.session_id).slice(0, 32))}
          </button>
          <button class="btn-mini" data-action="dismiss">Dismiss</button>
        </div>`;
      wrap.appendChild(row);
    }
    wrap.addEventListener("click", (e) => {
      const t = e.target.closest("[data-action]");
      if (!t) return;
      if (t.dataset.action === "dismiss") {
        t.closest(".recall-row").style.display = "none";
      } else if (t.dataset.action === "open") {
        // Hand off to the main app with the target session id.
        window.location.href = `index.html#open-replay=${encodeURIComponent(t.dataset.sid)}`;
      }
    });
  } catch (err) {
    wrap.innerHTML = `<div class="muted">recall queue 로드 실패: ${escapeHtml(String(err))}</div>`;
  }
}

// --------------------------------------------------------------------------
// Today's brief — pick latest session, fetch predict_next_actions
// --------------------------------------------------------------------------

async function loadTodayBrief() {
  const wrap = document.getElementById("todayBrief");
  if (state.sessions.length === 0) {
    wrap.innerHTML = `<div class="muted">최근 세션 없음.</div>`;
    return;
  }
  const latest = [...state.sessions]
    .sort((a, b) => (b.start_iso || "").localeCompare(a.start_iso || ""))[0];

  const lastAgo = relativeAgo(parseIso(latest.start_iso));
  const title = latest.ai_title || latest.project_name || latest.session_id;
  let predictHtml = `<div class="muted" style="font-size:11.5px">predict 계산 중…</div>`;

  wrap.innerHTML = `
    <p class="brief-line">
      마지막 세션: <strong>${escapeHtml(latest.project_name || "?")}</strong>
      · <span class="dim-2">${escapeHtml(title.slice(0, 60))}</span>
      · <span class="muted">${lastAgo}</span>
    </p>
    <p class="brief-line">
      turns ${latest.user_turns || 0}/${latest.assistant_turns || 0}
      · tool calls ${latest.tool_count || 0}
      · ${latest.has_errors ? "<span style='color:#ff6b8a'>has errors</span>" : "<span style='color:#6bd96b'>no errors</span>"}
    </p>
    <div id="todayPredict">${predictHtml}</div>`;

  try {
    const pred = await invoke("predict_next_actions", {
      sessionId: latest.session_id,
      lastNTurns: 3,
      horizon: 3,
      neighbors: 8,
    });
    renderPrediction(pred);
  } catch (err) {
    document.getElementById("todayPredict").innerHTML =
      `<div class="muted" style="font-size:11.5px">predict 실패: ${escapeHtml(String(err).slice(0,100))}</div>`;
  }
}

function renderPrediction(pred) {
  const wrap = document.getElementById("todayPredict");
  if (!wrap) return;
  const top = pred?.predictions?.[0];
  if (!top) {
    wrap.innerHTML = `<div class="muted" style="font-size:11.5px">예측 결과 없음 (이웃 세션 부족).</div>`;
    return;
  }
  const freq = Math.round((top.frequency || 0) * 100);
  wrap.innerHTML = `
    <div class="brief-pick">
      <div class="hd">Predicted next action · ${freq}% confidence</div>
      <div class="body">
        <span class="tk">${escapeHtml(top.tool_name)}</span>&nbsp;
        <span class="mono">${escapeHtml((top.example_input_summary || "").slice(0, 80))}</span>
        <br>
        <span class="muted" style="font-size:11px">
          based on ${pred.neighbors_used}/${pred.neighbors_searched} similar past sessions
          · last hit: ${escapeHtml(top.from_session_project || "?")} #${top.from_turn_index}
        </span>
      </div>
    </div>`;
}

// --------------------------------------------------------------------------
// Skill graph (mock layout — until per-turn tool extraction lands)
// --------------------------------------------------------------------------

function renderSkillGraph() {
  const svg = document.getElementById("skillGraph");
  if (!svg) return;
  svg.innerHTML = "";
  const N = [
    { id: "Bash",  x: 120, y: 110, f: 1.0 },
    { id: "Edit",  x: 210, y: 90,  f: 0.78 },
    { id: "Read",  x: 250, y: 150, f: 0.65 },
    { id: "Write", x: 170, y: 170, f: 0.48 },
    { id: "Grep",  x: 70,  y: 170, f: 0.32 },
    { id: "Task",  x: 310, y: 80,  f: 0.22 },
    { id: "Glob",  x: 50,  y: 80,  f: 0.16 },
    { id: "Web",   x: 310, y: 200, f: 0.08 },
  ];
  const E = [
    ["Bash","Edit", .95], ["Bash","Read", .88], ["Edit","Read", .82],
    ["Edit","Write", .75], ["Read","Grep", .68], ["Grep","Bash", .60],
    ["Edit","Glob", .42], ["Task","Bash", .55], ["Task","Edit", .40],
    ["Web","Read", .25], ["Write","Bash", .50],
  ];
  const byId = Object.fromEntries(N.map((n) => [n.id, n]));

  for (const [a, b, w] of E) {
    const A = byId[a], B = byId[b];
    const line = document.createElementNS("http://www.w3.org/2000/svg", "line");
    line.setAttribute("x1", A.x); line.setAttribute("y1", A.y);
    line.setAttribute("x2", B.x); line.setAttribute("y2", B.y);
    line.setAttribute("stroke", `rgba(90,166,255,${0.15 + 0.55 * w})`);
    line.setAttribute("stroke-width", 0.5 + w * 1.8);
    svg.appendChild(line);
  }
  for (const n of N) {
    const r = 6 + Math.sqrt(n.f) * 14;
    const g = document.createElementNS("http://www.w3.org/2000/svg", "g");
    const c = document.createElementNS("http://www.w3.org/2000/svg", "circle");
    c.setAttribute("cx", n.x); c.setAttribute("cy", n.y); c.setAttribute("r", r);
    c.setAttribute("fill", "rgba(116,224,193,.18)");
    c.setAttribute("stroke", "rgba(116,224,193,.7)");
    c.setAttribute("stroke-width", "1.2");
    g.appendChild(c);
    const t = document.createElementNS("http://www.w3.org/2000/svg", "text");
    t.setAttribute("x", n.x); t.setAttribute("y", n.y + r + 12);
    t.setAttribute("text-anchor", "middle");
    t.setAttribute("fill", "rgba(236,237,242,.8)");
    t.setAttribute("font-size", "10");
    t.setAttribute("font-family", "ui-monospace, monospace");
    t.textContent = n.id;
    g.appendChild(t);
    svg.appendChild(g);
  }
}

// --------------------------------------------------------------------------
// Week stats — real numbers off list_sessions
// --------------------------------------------------------------------------

function renderWeekStatsLoading() {
  document.getElementById("weekStats").innerHTML =
    `<div class="muted">집계 중…</div>`;
}

function renderWeekStats() {
  const wrap = document.getElementById("weekStats");
  wrap.innerHTML = "";
  const now = Date.now();
  const day = 86_400_000;
  const inWindow = (s, from, to) => {
    const t = parseIso(s.start_iso);
    return t && t.getTime() >= from && t.getTime() < to;
  };
  const thisWk = state.sessions.filter((s) => inWindow(s, now - 7 * day, now));
  const lastWk = state.sessions.filter((s) => inWindow(s, now - 14 * day, now - 7 * day));

  const sumTurns = (xs) => xs.reduce((a, s) => a + (s.user_turns || 0) + (s.assistant_turns || 0), 0);
  const sumTools = (xs) => xs.reduce((a, s) => a + (s.tool_count || 0), 0);
  const errCount = (xs) => xs.filter((s) => s.has_errors).length;

  const rows = [
    weekRow("Sessions", thisWk.length, lastWk.length),
    weekRow("Total turns", sumTurns(thisWk), sumTurns(lastWk)),
    weekRow("Tool calls", sumTools(thisWk), sumTools(lastWk)),
    weekRow("Sessions w/ errors", errCount(thisWk), errCount(lastWk)),
    weekRow("Distinct projects",
      new Set(thisWk.map((s) => s.project_name)).size,
      new Set(lastWk.map((s) => s.project_name)).size,
    ),
  ];
  for (const r of rows) wrap.appendChild(r);

  // Schedule the placeholder follow-ups so the "tool / error placeholder
  // body" is populated AFTER live data loads, not before.
  renderToolParetoFromSessions();
  renderErrorListFromSessions();
}

function weekRow(label, cur, prev) {
  const div = document.createElement("div");
  div.className = "weekstat";
  let txt = `${cur.toLocaleString()}`;
  let cls = "";
  if (prev > 0 || cur > 0) {
    const diff = cur - prev;
    const arrow = diff > 0 ? "↑" : diff < 0 ? "↓" : "↔";
    txt = `${arrow} ${cur.toLocaleString()} (was ${prev.toLocaleString()})`;
    cls = diff > 0 ? "up" : diff < 0 ? "down" : "";
  }
  div.innerHTML = `
    <span class="lbl">${escapeHtml(label)}</span>
    <span class="num ${cls}">${txt}</span>`;
  return div;
}

// --------------------------------------------------------------------------
// Utility
// --------------------------------------------------------------------------

function relativeAgo(date) {
  if (!date) return "—";
  const sec = Math.floor((Date.now() - date.getTime()) / 1000);
  if (sec < 60) return `${sec}s ago`;
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d}d ago`;
  const w = Math.floor(d / 7);
  if (w < 12) return `${w}w ago`;
  const mo = Math.floor(d / 30);
  return `${mo}mo ago`;
}

function escapeHtml(s) {
  return String(s ?? "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
function escapeAttr(s) { return escapeHtml(s).replace(/`/g, "&#96;"); }
