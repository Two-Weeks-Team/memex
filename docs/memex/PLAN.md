# Memex v2 — Implementation Plan

> **Goal**: Ship a working Memex.app for Qdrant Vector Space Day 2026 (deadline 2026-06-01).
> Today: 2026-05-18. Effective build window: **~13 days**. Target submission: **D-7 (May 25-28)** with buffer.

This document is the single source of truth for the next session. It contains:
1. Confirmed decisions
2. Stack
3. Phase-by-phase plan
4. Atomic TODO checklist (ralph_loop worklist)
5. CLI intervention points (user actions)
6. Test gates
7. ralph_loop prompt

---

## 1. Confirmed Decisions (2026-05-18)

| Decision | Choice | Rationale |
|---|---|---|
| **Tech stack** | **Tauri 2.x** (Rust shell + WebView frontend) | Real desktop app feel; smaller binary than Electron; cross-platform |
| **Repo** | **`memex`** standalone, fresh on personal GitHub | Clean Apache 2.0, no myproject/glasshat/Phoenix/Arize confusion |
| **Privacy** | **Full ~/.claude/projects scan** authorized | User confirmed; everything local |
| **Submission timing** | **ASAP** (target D-7 ≈ May 25, hard deadline 2026-06-01 23:59 PT) | Buffer for polish + demo recording |
| **Frontend** | Vanilla HTML/CSS/JS (port from `mockups/memex/demo.html` + `landing.html`) | No React/build step needed; iterate fast on existing mockup |
| **Embeddings** | Qdrant FastEmbed server-side (BGE-small-en-v1.5 + BM42 sparse + Jina ColBERT v2) | Zero Python deps; everything in Rust + Qdrant |
| **Sidecar** | None (Rust → qdrant-client direct) | Simpler than Python sidecar; FastEmbed handles inference |
| **Qdrant binary** | Bundled or `docker-compose` per platform | Whatever ships easier; README handles install |

---

## 2. Stack

```
┌─────────────────────────────────────────────────────┐
│  Tauri 2.x app (.app bundle on macOS)               │
│  ┌───────────────────────────────────────────────┐  │
│  │  Frontend (WebView)                           │  │
│  │  - Vanilla HTML/CSS/JS                        │  │
│  │  - Port from mockups/memex/                   │  │
│  │  - @tauri-apps/api for native calls           │  │
│  └────────────────┬──────────────────────────────┘  │
│                   │ tauri::invoke / events           │
│  ┌────────────────▼──────────────────────────────┐  │
│  │  Rust core                                    │  │
│  │  - qdrant-client (Rust)                       │  │
│  │  - walkdir + notify (file scan/watch)         │  │
│  │  - serde + tokio                              │  │
│  │  - petgraph (MST computation)                 │  │
│  └────────────────┬──────────────────────────────┘  │
└───────────────────┼──────────────────────────────────┘
                    │ HTTP/gRPC
┌───────────────────▼──────────────────────────────────┐
│  Qdrant local (binary or Docker)                    │
│  - 5 named vectors per session point                │
│  - FastEmbed for BGE-small + BM42 + ColBERT         │
│  - Binary Quantization + rescore                    │
│  - Payload-indexed (project, date, error_type)      │
└──────────────────────────────────────────────────────┘
```

Data flow:
1. Tauri reads `~/.claude/projects/**/*.jsonl` (user-approved)
2. Parses to sessions + turns + tool calls
3. Sends to Qdrant with FastEmbed (server embeds)
4. Frontend queries via Rust commands → Qdrant Universal Query API
5. Results render in webview

---

## 3. Phases

| Phase | Duration | Output | Test Gate |
|---|---|---|---|
| **0. Setup** | 0.5 day | `memex` repo + Tauri scaffold + Qdrant running | `cargo tauri dev` opens blank window; `curl localhost:6333` returns Qdrant version |
| **1. JSONL parser** | 1 day | Rust parser for Claude Code session jsonl | Parse 5 fixture files → expected turn counts |
| **2. Qdrant indexing** | 1.5 days | Bulk indexer; 5 named vectors; snapshot export | Index 50 fixtures; query returns reasonable top-5 |
| **3. 5 Qdrant features (API)** | 2 days | Rust commands for: Lens, Mix&Match, Topology, ColBERT, Snapshot | Each command returns valid JSON; manual curl test |
| **4. Frontend port (Tauri webview)** | 2.5 days | All v2 mockup UI wired to backend; stack/search/inspector/sidebar working | Click card → real session detail; ⌘K → real results |
| **5. Replay engine** | 1.5 days | Turn-by-turn playback with Bash/Edit/Read tool viz | Click Replay on real session → animates correctly |
| **6. Proactive recall** | 1 day | File watcher + recall banner | Tail-watch fixture jsonl; pattern match → banner |
| **7. macOS polish** | 1 day | Tray icon, permissions prompt, auto-launch, .app build | `tauri build` produces installable .app |
| **8. Demo + README + submit** | 1.5 days | 3-min demo video + README + GitHub public + VSD form | User submits, gets confirmation |

**Total: ~12 days. Buffer ~2 days for unknowns.**

---

## 4. TODOS (ralph_loop worklist)

Atomic tasks. ralph_loop picks next unchecked, executes, marks done, loops.

### Phase 0 — Setup
- [x] T0.1: `gh repo create memex --public --license apache-2.0` — `sgwannabe/memex` (created 2026-05-17)
- [x] T0.2: `gh repo clone sgwannabe/memex ~/memex` — clean clone at `~/memex`
- [x] T0.3: Rust toolchain present (cargo 1.88.0, rustc 1.88.0) — no install needed
- [x] T0.4: Tauri CLI via `@tauri-apps/cli@^2` (project-local from `npm install`)
- [x] T0.5: `npm create tauri-app@latest -- --yes --template vanilla --manager npm --identifier dev.sgwannabe.memex` → rsynced into `~/memex/` (LICENSE preserved)
- [x] T0.6: `cargo check` PASS in 36.95s — full dep graph compiles (Tauri 2.11.2 + tauri-plugin-opener 2.5.4 + serde 1). Visual window verification deferred to user via `npm run tauri dev`.
- [x] T0.7: Qdrant v1.18.0 (aarch64-apple-darwin) at `~/memex/.qdrant/qdrant`, running, storage `~/memex/storage/`
- [x] T0.8: `curl localhost:6333` → `{"title":"qdrant - vector search engine","version":"1.18.0"}` ✅
- [x] T0.9: `demo.html` (100K) + `landing.html` (43K) copied from `myproject/workspace-b/mockups/memex/` → `~/memex/src/`
- [x] T0.10: Commit "scaffold: tauri + qdrant + mockup port"

### Phase 1 — JSONL parser ✅ (2026-05-18)
- [x] T1.1: Deps added: `serde`, `serde_json`, `walkdir`, `chrono`, `anyhow`, `thiserror`, `clap` (+ `pretty_assertions` dev-dep)
- [x] T1.2: `Session` + `Turn` + `ToolCall` + `ToolResult` + `TurnRole` + `EventCounts` in `parser.rs`
- [x] T1.3: `parse_session(&Path) -> Result<Session>` — handles `user`/`assistant`/`system` event types, string + array content, `tool_use`/`tool_result`/`text` items
- [x] T1.4: `scan_dir(&Path) -> Vec<Session>` — walks recursively, skips `subagents/` traces
- [x] T1.5: 5 fixtures in `src-tauri/tests/fixtures/` (minimal, tool_use+result, ai-title+metadata, tool_error, mixed sidechain) + extra subagent-skip fixture set
- [x] T1.6: `cargo test --test parser` → 8/8 pass in 28s (compile) + 0ms (tests)
- [x] T1.7: `memex scan --limit 10` against `~/.claude/projects` parsed 80 real sessions, 17,706 tool calls, 0 errors
- [x] T1.8: Commit "phase 1: jsonl parser + tests"

### Phase 2 — Qdrant indexing
- [ ] T2.1: Add deps: `qdrant-client`, `tokio`
- [ ] T2.2: Define Qdrant collection schema (5 named vectors + payload index)
  - `content` (dense 384d BGE-small)
  - `tool` (dense 384d BGE-small) — text of all tool calls
  - `path` (sparse BM42) — file paths mentioned
  - `error` (dense 384d) — error text only
  - `code` (dense 384d) — code blocks only
- [ ] T2.3: `init_collection()` creates collection if missing
- [ ] T2.4: `upsert_session(session)` builds 5 vectors via FastEmbed, calls Qdrant
- [ ] T2.5: Bulk indexer with progress bar (`indicatif`)
- [ ] T2.6: ColBERT (Jina-ColBERT-v2) sentence-level multi-vector — separate collection or as 6th named vec
- [ ] T2.7: Snapshot CLI: `memex snapshot export` and `memex snapshot import`
- [ ] T2.8: Verify: index 50 fixtures, query "modal IP" returns expected hit
- [ ] T2.9: Commit "phase 2: qdrant indexing + snapshot"

### Phase 3 — 5 Qdrant features (Rust commands exposed to Tauri)
- [ ] T3.1: `tauri::command lens_search(query, weights: {content, tool, path, error, code})` — Universal Query API multi-stage
- [ ] T3.2: `tauri::command mix_and_match(positive_ids, negative_ids)` — Discovery API multi-pair context
- [ ] T3.3: `tauri::command topology()` — Distance Matrix API → petgraph MST → return JSON {nodes, edges}
- [ ] T3.4: `tauri::command colbert_explain(session_id, query)` — find matching sentence with offset
- [ ] T3.5: `tauri::command snapshot_export(path)` and `snapshot_import(path)`
- [ ] T3.6: `tauri::command recall(error_signature)` — proactive recall match
- [ ] T3.7: Verify each command via `tauri dev` → console manual invoke
- [ ] T3.8: Commit "phase 3: 5 qdrant features"

### Phase 4 — Frontend port (Tauri webview)
- [ ] T4.1: Update mockup HTML to use Tauri's `@tauri-apps/api` instead of hardcoded data
- [ ] T4.2: Wire ⌘K search to `lens_search` command
- [ ] T4.3: Wire Lens slider inspector to weights state → re-query on change
- [ ] T4.4: Wire Mix & Match drop zones → `mix_and_match` command on Run
- [ ] T4.5: Wire Topology toggle → `topology` command → render SVG MST
- [ ] T4.6: Wire ColBERT inline citation → `colbert_explain` on result click
- [ ] T4.7: Wire Snapshot button → `snapshot_export` with file picker
- [ ] T4.8: Wire Card click → load session detail in inspector
- [ ] T4.9: Time Machine stack: wheel/arrow nav → setLayer state → re-layout (already in mockup)
- [ ] T4.10: Verify: open app, search, lens-adjust, mix, topology, snapshot all work end-to-end
- [ ] T4.11: Commit "phase 4: frontend wired to backend"

### Phase 5 — Replay engine
- [ ] T5.1: `tauri::command get_session_turns(session_id)` returns all turns with tool details
- [ ] T5.2: Replay UI: split view (turn list + turn detail) — already in mockup
- [ ] T5.3: Tool visualizations: Bash terminal, Edit diff, Read snippet, WebFetch URL preview, Task spawn
- [ ] T5.4: Playback timing: configurable speed (1x / 2x / 4x / 8x default)
- [ ] T5.5: Scrub control + turn-list click → jump to specific turn
- [ ] T5.6: Verify on real session: pick one from user's actual ~/.claude/projects, click Replay
- [ ] T5.7: Commit "phase 5: replay engine"

### Phase 6 — Proactive recall
- [ ] T6.1: File watcher (`notify` crate) on `~/.claude/projects` for new file events
- [ ] T6.2: Detect new turn appended → parse last 2-3 turns for error patterns
- [ ] T6.3: If error signature matches past session via `recall` command → emit Tauri event
- [ ] T6.4: Frontend listens for event → slide in recall banner
- [ ] T6.5: Banner click → open the past session's Replay at the fix turn
- [ ] T6.6: Verify: induce a known error pattern in fixture, watch banner appear
- [ ] T6.7: Commit "phase 6: proactive recall"

### Phase 7 — macOS polish
- [ ] T7.1: App icon design (use Memex SVG or similar minimal)
- [ ] T7.2: Tray icon + minimal menu (Open Memex, Snapshot, Quit)
- [ ] T7.3: Full Disk Access permission prompt (macOS-specific) for ~/.claude
- [ ] T7.4: `tauri build` produces .app + .dmg
- [ ] T7.5: Verify .app launches from Finder without errors
- [ ] T7.6: Commit "phase 7: macos polish + .app build"

### Phase 8 — Demo + README + submit
- [ ] T8.1: Record 3-min demo video showing all 5 features + Replay (user-driven)
- [ ] T8.2: Write README.md: install, scan, search, replay, snapshot. Include screenshots.
- [ ] T8.3: Add LICENSE (Apache 2.0)
- [ ] T8.4: Add architecture.md with diagrams (lift from this doc + mockups)
- [ ] T8.5: Add docs/qdrant-features.md explaining all 5 Qdrant-unique features
- [ ] T8.6: `gh repo edit --add-topic qdrant --add-topic vector-search`
- [ ] T8.7: Upload demo video to YouTube (unlisted OK), embed in README
- [ ] T8.8: User submits to VSD via Google Form (https://forms.gle/YDQ2TDUi8MqS9Vx28)
- [ ] T8.9: Commit "phase 8: ready for submission"

---

## 5. CLI Intervention Points (User Actions)

ralph_loop will PAUSE and request user when reaching these. Each is short (<10 min).

| # | When | Command/Action | Why user-only |
|---|---|---|---|
| 1 | T0.1 (Phase 0 start) | `gh repo create memex --public --license apache-2.0` | Needs GitHub auth |
| 2 | T0.3 (if Rust missing) | `curl ... \| sh` then restart shell | One-time install |
| 3 | T0.7 (Qdrant install) | Choose Docker (`docker pull qdrant/qdrant`) or binary download | User preference + sudo possibly |
| 4 | T2.8 (after first index) | Spot-check: "does query 'modal IP' return myproject session?" | Verify privacy/correctness on real data |
| 5 | T4.10 (after frontend wired) | Click through all 5 features, confirm no UI glitches | Visual review |
| 6 | T5.6 (after replay) | Verify replay on YOUR session looks right | Privacy double-check + UX review |
| 7 | T7.3 | Grant Full Disk Access in System Settings | OS prompt, manual click |
| 8 | T8.1 | Record demo video (screen capture, ~1 hour) | Cannot automate |
| 9 | T8.8 | Submit to VSD Google Form | Cannot automate (form) |

ralph_loop expects user to be available for ~9 brief check-ins over ~12 days.

---

## 6. Test Gates

Each phase ends with a hard gate. ralph_loop runs the gate test; if fail, retries until pass.

| Phase | Gate command | Pass criterion |
|---|---|---|
| 0 | `cargo tauri dev` opens window AND `curl localhost:6333 \| jq .title` | Window opens, returns `"qdrant - vector search engine"` |
| 1 | `cargo test --test parser` | All fixture parses pass |
| 2 | `cargo run -- scan --index ~/.claude/projects && cargo run -- search "modal IP"` | Returns ≥1 result matching myproject |
| 3 | Each command via `tauri dev` console: `invoke('lens_search', {...})` | Returns valid JSON, latency <50ms p99 |
| 4 | Manual click-through: search → lens → mix → topology → snapshot | All work without console errors |
| 5 | Click Replay on real session | Turns animate, tool viz renders |
| 6 | Append a known error to fixture jsonl | Banner appears within 2s |
| 7 | `tauri build` succeeds, double-click `.app` | App launches without dialog errors |
| 8 | README rendered + video embedded + repo public | `gh repo view --web` shows complete repo |

---

## 7. ralph_loop Prompt

Use this exact prompt to start ralph_loop in the new session:

```
You are implementing Memex v2 per docs/memex/PLAN.md in the myproject worktree.

The plan defines 8 phases with atomic TODO checkboxes (T0.1 ... T8.9).
ralph_loop iterates through them.

EACH ITERATION:
1. Read docs/memex/PLAN.md
2. Find the FIRST unchecked task (T*.* with [ ])
3. If it's a "user CLI intervention" task (see §5):
   - Print the exact command + reason
   - STOP the loop with a clear "AWAITING USER" message
   - Wait for user to confirm completion before next iteration
4. Otherwise, execute the task:
   - Write/edit files as needed
   - Run tests if applicable
   - Verify gate passes if end of phase
5. Mark task as [x] in PLAN.md
6. Commit with message "phase X: task T*.* — <brief>"
7. Continue to next iteration

CONSTRAINTS:
- ALL work is in ~/memex (the new repo), NOT in myproject
- Reference myproject's mockups/memex/{demo,landing}.html as canonical UI
- Use Tauri 2.x + Rust + Qdrant FastEmbed (no Python sidecar)
- Test gates are hard: if fail, debug + retry until pass before moving on
- If stuck >30 minutes on one task, STOP and ask user

STARTING STATE:
- myproject worktree has all decisions + mockups
- ~/.claude/projects exists with real session data (user authorized scan)
- Today: 2026-05-18. Target submission: ~May 25-28.

START: Pick T0.1 and proceed.
```

---

## 8. Handoff Instructions for New Session

### Option A — Recommended: Fresh Claude Code session

In a NEW Claude Code session (preferably with `cd ~/memex` after T0.1-T0.2):

1. Read this file: `docs/memex/PLAN.md` (or symlink to it from the new repo)
2. Run `/handon` if available, OR paste the ralph_loop prompt from §7 above
3. Let ralph_loop iterate through tasks

The new session will:
- Have full context budget (this session is near limit)
- Execute deterministically per checklist
- Pause at user-CLI tasks (§5)

### Option B — Continue in this session

NOT recommended due to context state. If you must:

1. Run auto-compaction (if available)
2. Then paste the ralph_loop prompt
3. Risk: prompt drift, lost decisions

### Recovery / debugging

If ralph_loop gets stuck or wanders:

1. `git diff` to see what changed
2. Revert if needed: `git checkout -- .`
3. Re-read this PLAN.md
4. Resume from last unchecked task

---

## 9. References (from myproject worktree)

For the new session, these are READ-ONLY references (do not modify myproject):

| Reference | Purpose |
|---|---|
| `mockups/memex/demo.html` | Canonical UI for the app — port to Tauri webview |
| `mockups/memex/landing.html` | Landing/website (deploy to GitHub Pages later) |
| `docs/memex/PLAN.md` | This file (single source of truth) |
| `data/devpost-gemini3/` | NOT NEEDED for Memex (was Atlas-era) |
| `spikes/09_signal_validation/` | NOT NEEDED for Memex (Atlas negative result) |

For the new repo (`memex`), structure should be:

```
memex/
├── src-tauri/              # Rust core
│   ├── src/
│   │   ├── main.rs
│   │   ├── parser.rs       # JSONL → Session structs
│   │   ├── indexer.rs      # Qdrant upsert + FastEmbed
│   │   ├── commands/
│   │   │   ├── lens.rs
│   │   │   ├── mix.rs
│   │   │   ├── topology.rs
│   │   │   ├── colbert.rs
│   │   │   ├── snapshot.rs
│   │   │   └── recall.rs
│   │   ├── watcher.rs      # File watcher
│   │   └── lib.rs
│   ├── Cargo.toml
│   └── tauri.conf.json
├── src/                    # Frontend (vanilla HTML/CSS/JS)
│   ├── index.html          # Time Machine main view
│   ├── styles.css
│   └── app.js
├── tests/
│   └── fixtures/           # 5 sample sessions
├── README.md
├── LICENSE                 # Apache 2.0
├── docs/
│   ├── architecture.md
│   └── qdrant-features.md
└── .gitignore
```

---

## 10. Risk Register

| Risk | Probability | Impact | Mitigation |
|---|---|---|---|
| qdrant-client Rust crate missing some 1.10+ APIs (Discovery multi-pair, Distance Matrix) | Medium | High | Fallback: REST API direct via `reqwest`; or Python sidecar |
| FastEmbed doesn't support Jina ColBERT v2 server-side | Medium | Medium | Fallback: precompute embeddings client-side via `ort` Rust crate, upload as multi-vector |
| Tauri 2.x file watcher unreliable on macOS | Low | Medium | Fallback: polling every 2s |
| Demo video recording quality | Low | Medium | Use macOS built-in screen recording; do 2 takes |
| VSD form changes | Very Low | Low | Verify URL before submission |
| ~/.claude/projects format change | Low | Medium | Version-detect parser; ship support for both Claude Code and Codex |

---

## 11. Done = Submission Ready

Defined as:
- ✅ `memex` GitHub repo public with Apache 2.0
- ✅ README explains install + usage + screenshots
- ✅ All 5 Qdrant features working end-to-end on real data
- ✅ Replay works on real sessions
- ✅ Proactive recall works on file watch
- ✅ `.app` bundle builds and runs
- ✅ 3-min demo video on YouTube
- ✅ User submitted to https://forms.gle/YDQ2TDUi8MqS9Vx28

When all green: declare victory.

---

*Plan written 2026-05-18 in myproject worktree. Single source of truth for next session.*
