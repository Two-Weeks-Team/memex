# Implementation Plan — Active Memex (MCP + Notifications + Auto-Index)

> Ralph-loop ready. Each task is atomic, has a clear gate, and ends with commit.
> The loop: read this file → find first `[ ]` → execute → mark `[x]` → commit → next.

## Goal

Turn Memex from a passive viewer into an active layer that any MCP-aware AI agent (Claude Code, Codex, Cursor, …) can consult, and that proactively pushes macOS notifications when something relevant happens. **Memex stands alone — zero dependency on any third-party project; no integration with anything beyond the standard MCP spec.**

## Constraints

- Memex remains 100% local. No new network calls.
- The macOS `.app` bundle still launches the same way.
- The existing CLI subcommands (`scan`, `search`, `lens`, …) keep working unchanged.
- All new Rust deps must compile clean with `cargo build` + 0 new clippy warnings.
- Parser unit tests (`cargo test --test parser`) must keep passing.

## Modules

### M1 · MCP server (stdio JSON-RPC) — 9 tools

- [x] T1.1: Add `rmcp = "0.7"` (or current latest stable) to `src-tauri/Cargo.toml`. If `rmcp` compile fails, fall back to hand-rolled stdio JSON-RPC 2.0 with Content-Length framing per the MCP spec.
- [x] T1.2: New module `src-tauri/src/mcp.rs`. Public entrypoint `pub async fn run() -> anyhow::Result<()>` that starts the stdio server.
- [x] T1.3: Implement the MCP handshake — `initialize`, `notifications/initialized`, server info (name=`memex`, version from `CARGO_PKG_VERSION`), capabilities = `{ "tools": {} }`.
- [x] T1.4: Implement `tools/list` returning the catalog of 9 tools below.
- [x] T1.5: Implement `tools/call` dispatch. Each tool reuses the existing `indexer::*` functions through a shared `Embedder` + `Qdrant` (lazy-init same as commands.rs).
- [x] T1.6: 9 tools (matching `docs/qdrant-features.md`):
  - `find_similar_sessions(query: string, limit?: int, weights?: object)` → SearchHit[]
  - `find_similar_error(error_text: string, limit?: int)` → SearchHit[]
  - `predict_next_action(session_id: string, last_n?: int, horizon?: int, neighbors?: int)` → PredictionContext
  - `mix_similar_sessions(positive: string[], negative: string[], limit?: int)` → SearchHit[]
  - `get_session_summary(session_id: string)` → payload JSON
  - `get_session_turn(session_id: string, turn_index: int)` → turn JSON
  - `list_recent_sessions(limit?: int, project_filter?: string)` → SessionSummary[]
  - `analyze_corpus_topology()` → Topology (with project_insights + gap_insights)
  - `snapshot_export(path: string)` → server-side snapshot name
- [x] T1.7: Wire `memex mcp` CLI subcommand → calls `mcp::run()`. Add to `CLI_SUBCOMMANDS` in `main.rs`.
- [x] T1.8: Smoke test: `echo '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' | memex mcp` returns 9-tool catalog as a valid JSON-RPC response (after `initialize`).
- [x] T1.9: Install helper — new CLI subcommand `memex install-mcp` that prints (and optionally runs) `claude mcp add memex <binary-path> mcp`. Document in README.
- [x] T1.10: Verify with `claude mcp add memex /full/path mcp && claude mcp list` shows memex ✓.
- [x] T1.11: Commit: `feat: MCP server + 9 tools — Memex as agent memory layer`.

### M2 · Auto-index daemon (background)

- [x] T2.1: New module `src-tauri/src/watcher.rs`. Function `pub async fn start_watcher(state: AppStateArc, app_handle: tauri::AppHandle, root: PathBuf, period: Duration)`.
- [x] T2.2: Inside `start_watcher`: tokio task that loops every `period` seconds. Walk `root`, find `.jsonl` files modified since the last tick, re-parse + `index_session` each. Skip `subagents/` dir.
- [x] T2.3: Per-tick stats: `{ checked, reindexed, new, errors }`. Cache last-seen `mtime` per path in a `Mutex<HashMap<PathBuf, SystemTime>>` so we don't re-upsert unchanged files.
- [x] T2.4: Emit Tauri event `index-updated` with the stats payload whenever `reindexed > 0`.
- [x] T2.5: Spawn the watcher from `lib.rs::setup` AFTER `AppState` is managed. Default `root = $HOME/.claude/projects`, `period = 60 s`.
- [x] T2.6: Frontend `main.js` listens to `index-updated` → updates the topbar count + status bar (`"Re-indexed 3 session(s)"` fade-in chip).
- [x] T2.7: Commit: `feat: background auto-index daemon — mtime-keyed incremental upserts`.

### M3 · macOS notifications

- [x] T3.1: Add `tauri-plugin-notification = "2"` to `src-tauri/Cargo.toml`. Add the plugin's capability JSON.
- [x] T3.2: Register the plugin in `lib.rs::run()`: `.plugin(tauri_plugin_notification::init())`.
- [x] T3.3: On the first run of the watcher (or first launch), call `notification.request_permission()` from Rust.
- [x] T3.4: New Rust helper `commands::notify_recall(app: &AppHandle, ev: &RecentError, hits: &[SearchHit])` — emits a system notification `Memex · I've seen this error before · <project> · turn #<idx>`.
- [x] T3.5: Extend the watcher: when a fresh `tool_result.is_error` is detected on a session and `recall(error_text)` returns a hit with score ≥ 0.65 in a DIFFERENT session, fire `notify_recall`. Debounce: 1 hour per (session_id, error_text-prefix) pair.
- [x] T3.6: Click handler on the notification: opens the main webview window + emits a Tauri event `open-replay-from-notification` with `{ session_id, turn_index }`. Frontend handles → calls `openReplay(...)` and jumps to that turn.
- [x] T3.7: Predict-based notification (stretch — only if T3.6 lands cleanly): if a live session's last-3-turn embedding closely matches a past session AND `predict_next_action` returns top-1 with frequency > 0.7 AND that tool hasn't yet been called by the live session, fire `Memex · past-you ran <tool> next 80% of times`. Debounce 30 min.
- [x] T3.8: Commit: `feat: macOS notifications — proactive recall + predict alerts`.

### M4 · Polish + verify

- [x] T4.1: `cargo clippy --all-targets` returns 0 warnings.
- [x] T4.2: `cargo test --test parser` 8/8 pass.
- [x] T4.3: Manual end-to-end:
  - CLI: `echo '{...initialize...}' | memex mcp` → handshake works.
  - `claude mcp add memex …` + `claude mcp list` → memex ✓.
  - In a Claude Code session: ask "find similar past sessions to X" → Claude invokes `find_similar_sessions` and returns hits.
  - Memex.app boots → watcher logs `[memex] watcher started …` to stderr.
  - Touch a fixture jsonl with a fake error → recall notification fires within ~60 s.
- [x] T4.4: Update README: add an `## 🧠 MCP integration` section after the features table — list the 9 tools + `claude mcp add memex …` one-liner + a screenshot/code-block of an example transcript.
- [x] T4.5: Update landing page (`index.html`): add a small "Works as an MCP server — connect any AI agent" badge under the hero CTAs, with a `<details>` block expanding the `claude mcp add` command.
- [x] T4.6: `npm run tauri build` produces `Memex.app` + `.dmg` clean.
- [x] T4.7: Commit final: `chore: docs + landing — MCP integration story`.
- [x] T4.8: Push to origin/main.

## Done definition

All boxes above checked. README + landing reflect the new MCP surface. `Memex.app` rebuilt. `claude mcp list` shows memex ✓ on the dev machine.

## Notes for the loop

- If `rmcp` crate doesn't exist / doesn't compile in T1.1, fall back to hand-rolled JSON-RPC 2.0. Don't waste loops fighting the framework — the spec is small.
- If `tauri-plugin-notification` rejects on the bundled `.app` because of missing permissions, document the System Settings step in README and ship anyway.
- Do NOT introduce any reference to third-party projects (Serena etc.) in code, README, landing, or commit messages. Memex is a standalone Qdrant-VSD entry.
- If context runs out mid-loop, commit whatever compiles + the current state of this checklist. Next session resumes from the first unchecked task.
