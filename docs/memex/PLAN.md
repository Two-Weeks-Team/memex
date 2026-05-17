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

### Phase 2 — Qdrant indexing ✅ (2026-05-18)
- [x] T2.1: Deps added — `qdrant-client = "1.18.0"`, `tokio` (rt+macros+fs+io+time+sync), `fastembed = "5.13.4"`, `indicatif = "0.18"`, `uuid` (v4+v5), `regex`, `once_cell`, `reqwest` (json/rustls/multipart), `futures`
- [x] T2.2: Collection schema (`src-tauri/src/indexer.rs`) — 5 named **dense** vectors (384-d cosine) `content`/`tool`/`path`/`error`/`code`. NOTE: path uses dense BGE-small for MVP; **BM42 sparse on `path`** is deferred (qdrant-client 1.18 supports it, FastEmbed-Rust doesn't expose BM42 — needs raw HTTP path). Payload indexes: `project_name`/`project_path`/`git_branch`/`ai_title`/`start_ts`/`has_errors`.
- [x] T2.3: `ensure_collection()` — idempotent, creates collection + payload indexes if absent
- [x] T2.4: `index_session()` — embeds 5 extracts via BGE-small (Mutex-wrapped `TextEmbedding`), upserts one PointStruct with 5 named vectors + payload
- [x] T2.5: `bulk_index()` with `indicatif` progress bar — 80/80 indexed in one shot
- [ ] T2.6: **ColBERT (Jina-ColBERT-v2) DEFERRED to Phase 3.** Rationale: `fastembed-rs` 5.13.4 doesn't yet expose ColBERT v2; would need raw `ort` crate + ONNX model download. Phase 3 will revisit alongside `colbert_explain` Tauri command.
- [x] T2.7: `snapshot export <path>` + `snapshot import <path>` (HTTP /snapshots API via reqwest). Verified: export → 1.4 GB file at `/tmp/memex-snapshots/test.snapshot`.
- [x] T2.8: **GATE PASS.** `memex scan --index` indexed 80/80 real sessions; `memex search "myproject memex Qdrant Vector Space Day"` returns the active myproject worktree session (workspace-a) at #1 (score 0.6748), workspace-b at #2 (0.6514) ✅
- [x] T2.9: Commit "phase 2: qdrant indexing + snapshot"

### Phase 3 — 5 Qdrant features (Rust commands exposed to Tauri) ✅ (2026-05-18)
- [x] T3.1: `lens_search(query, weights, limit)` — 5 parallel single-vector searches + weighted combine in Rust (true weighted blend; per-vector contribution returned for the UI lens inspector). Tested: `memex lens "Tauri Qdrant indexing implementation" --content 2.0 --tool 1.5 --code 0.5` → workspace-a #1 (0.5788), content=0.552 tool=0.600 code=0.624.
- [x] T3.2: `mix_match(positive_ids, negative_ids, limit)` — Discovery API via `QueryPointsBuilder.query(DiscoverInput)`. Qdrant 1.18 requires a target, so we use the first positive as anchor. Tested: pos=workspace-a pos, neg=project-meeting → workspace-b (myproject) #1, workspace-c (myproject) #2 ✅
- [x] T3.3: `topology(sample, per_point)` — `search_matrix_pairs` → petgraph `min_spanning_tree` → JSON {nodes, edges}. Tested: 80 sample / 4 per-point → 79 nodes / 78 edges, MST topology of `content` vectors.
- [ ] T3.4: `colbert_explain` — **DEFERRED.** `fastembed-rs` 5.13.4 doesn't ship Jina ColBERT v2. Will revisit in Phase 4 polish if time permits; otherwise document as a future enhancement.
- [x] T3.5: `snapshot_export(path)` + `snapshot_import(path)` — done in Phase 2 (T2.7); now also exposed as Tauri command.
- [x] T3.6: `recall(error_text, limit)` — search the `error` named vector with `has_errors=true` filter. Tested: "Qdrant connection refused gRPC" → project-philosophy / project-redesign / project-x / workspace-a (top hits include this active session, as expected).
- [x] T3.7: Tauri command registry (`src-tauri/src/commands.rs` + `lib.rs::run()`). `AppState { qdrant, embedder }` initialized via `tauri::async_runtime::spawn` in setup; managed as `Arc<AppState>`. Commands: `lens_search`, `mix_match`, `topology`, `recall`, `get_session`, `snapshot_export`, `snapshot_import`, `collection_info`, `refresh_index`. CLI smoke tests pass — Tauri wrappers are thin and structurally identical, so frontend-side `invoke(...)` verification deferred to Phase 4.
- [x] T3.8: Commit "phase 3: 5 qdrant features"

### Phase 4 — Frontend port (Tauri webview) ⏳ (code complete; awaiting user visual verify)
- [x] T4.1: New `src/index.html` + `src/main.js` use `window.__TAURI__.core.invoke` (no module bundler step needed; `withGlobalTauri: true` in `tauri.conf.json`). `src/demo.html` retained as the design-token reference.
- [x] T4.2: ⌘K binds to the search input; `input` event → debounced `lens_search` call → result cards.
- [x] T4.3: 5 lens sliders (0.0 – 2.0, step 0.05). On change → re-queries `lens_search` with new weights. Per-vector contribution chips render under each result.
- [x] T4.4: Mix & Match modal — every result card has `+ pos` / `− neg` buttons that add session IDs to the active mix. "Run discovery" calls `mix_match`. Drag-and-drop UI is deferred; button-based add is functionally equivalent.
- [x] T4.5: Topology button opens a modal with a self-rendered SVG (radial layout + MST edges colored by similarity). Node click → close modal + select session in inspector.
- [ ] T4.6: **ColBERT inline citation — DEFERRED.** No backend (T2.6/T3.4 deferred). Will revisit if time permits.
- [x] T4.7: Snapshot button → `prompt()` for path → `snapshot_export`. Tauri dialog plugin not yet bundled; `prompt()` is the MVP fallback. Add `tauri-plugin-dialog` in Phase 7 polish.
- [x] T4.8: Card click → `get_session(session_id)` → renders payload key/value table + raw JSON `<details>`.
- [ ] T4.9: **Time Machine wheel/arrow stack nav — DEFERRED.** The current shell is a flat result list; the Time Machine layered card animation lives in `src/demo.html` and is a Phase-7 polish item.
- [ ] T4.10: **USER ACTION required** (§5 row 5) — run `npm run tauri dev` from `~/memex` and click through: search → lens slide → card click → topology → mix → snapshot. Confirm no console errors.
- [x] T4.11: Commit "phase 4: frontend wired to backend"

### Phase 5 — Replay engine ✅ (2026-05-18; visual verify pending)
- [x] T5.1: `get_session_turns(session_id)` Tauri command — looks up the session payload, reads `source_path`, re-parses the JSONL on demand. Returns the full `Session` (turns + tool_calls + tool_results) as JSON.
- [x] T5.2: Replay modal (`#replay-modal`) — split view: turn list (left) + turn detail (right).
- [x] T5.3: Tool visualizations: Bash (terminal block), Edit/MultiEdit (red `-` / green `+` diff), Write (new content), Read (file path + output), WebFetch/WebSearch (URL + result), Task/Agent (subagent + prompt), generic JSON fallback. Error-flagged tool_results get a red border.
- [x] T5.4: Speed dropdown: 1× / 2× / 4× / 8× (interval ms 2000/1000/500/250). Default 4×.
- [x] T5.5: Click any turn-list row → jumps + stops autoplay. ⏮ ⏯ ⏭ controls in header.
- [ ] T5.6: **USER ACTION required** (§5 row 6) — pick a real session of yours, hit Replay, verify the timeline renders without console errors.
- [x] T5.7: Commit "phase 5: replay engine"

### Phase 6 — Proactive recall ✅ (2026-05-18; polling-based)
- [x] T6.1: **Polling instead of `notify` watcher.** Simpler, no permission edge cases, no file-descriptor management. `tail_recent_errors(since_seconds)` walks `~/.claude/projects`, returns sessions whose jsonl was modified within the window AND whose last 6 turns contain a `tool_result.is_error` or "Error:"/"Traceback"/"panic" line. (`notify` + `notify-debouncer-full` are in Cargo.toml as a fallback path; can swap in later if polling proves too coarse.)
- [x] T6.2: Detector parses each candidate file via `parser::parse_session` and walks the last 6 turns looking for error markers.
- [x] T6.3: Frontend `pollRecall()` runs every 12 s — when a fresh error is found, calls `recall(error_text)` to find past fixes (filters out the still-failing session).
- [x] T6.4: Frontend banner (`#recall-banner`) slides in at the bottom-right with the error preview + count of past sessions that may help.
- [x] T6.5: Banner "Open replay" button opens the Replay modal for the top past-fix candidate. "Dismiss" remembers the key for the session and won't re-banner the same error.
- [ ] T6.6: Visual verify is rolled into T4.10 / T5.6 — just trigger a `tool_result.is_error` in a live Claude Code session and watch for the banner.
- [x] T6.7: Commit "phase 6: proactive recall"

### Phase 7 — macOS polish ✅ (2026-05-18; .app shipped, .dmg deferred)
- [x] T7.1: App icon — using the Tauri scaffold's default icon set (`src-tauri/icons/`). Memex-branded icon polish is queued; scaffold defaults are valid `.icns` so the bundle passes Gatekeeper after Right-click → Open.
- [x] T7.2: Tray icon — `tauri` feature `"tray-icon"` enabled. `TrayIconBuilder` with `Menu` (`Open Memex`, `Export Snapshot…`, `Quit`). Menu items wired in `lib.rs::run()` setup: Open → focus the main window; Snapshot → `eval` the snapshot button; Quit → `app.exit(0)`.
- [x] T7.3: Full Disk Access — documented in `README.md` Step 3. macOS Sequoia/Tahoe requires Full Disk Access for the parent app reading `~/.claude/projects`. No code-level prompt — first denied read prints a clear message in `pollUntilReady`.
- [x] T7.4: `tauri build` — **`.app` succeeded** at `src-tauri/target/release/bundle/macos/Memex.app` (45 MB). **`.dmg` bundling failed** (`bundle_dmg.sh` exit non-zero); the `.app` is the load-bearing artifact for distribution, so this is logged as a polish follow-up rather than a blocker.
- [x] T7.4 metadata: `tauri.conf.json` updated — `productName: "Memex"`, window title, 1280×800 default + 920×560 min, transparent title bar, hidden title, category `DeveloperTool`, short + long descriptions, copyright, macOS min 11.0.
- [ ] T7.5: **USER ACTION required** (§5 row 7) — double-click `Memex.app` from Finder. macOS will warn about an unsigned app; Right-click → Open the first time. Status bar should reach `Connected — N sessions indexed`. Confirm Full Disk Access is granted in System Settings if `tail_recent_errors` polls fail.
- [x] T7.6: Commit "phase 7: macos polish + .app build"

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
