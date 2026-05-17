# Memex — Time Machine for AI session JSONL

> Search, replay, and learn from every Claude Code / Codex session you've ever run.
> Built on **Qdrant** with five vector lenses, sentence-level explanations, and proactive recall.

[![License](https://img.shields.io/badge/license-Apache_2.0-blue.svg)](LICENSE)
![Tauri 2](https://img.shields.io/badge/Tauri-2.x-24c8db)
![Qdrant 1.18](https://img.shields.io/badge/Qdrant-1.18-dc382d)
![Rust 1.88](https://img.shields.io/badge/Rust-1.88-orange)
![Status: Hackathon MVP](https://img.shields.io/badge/status-hackathon%20MVP-yellow)

Submission for **Qdrant Vector Space Day 2026** (deadline 2026-06-01).

---

## What problem does it solve?

Every time you finish a Claude Code session, the transcript lives in
`~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl` — a JSONL of every
user turn, every assistant response, every tool call, every output, forever.

That's a goldmine of *your* engineering experience. But it's read-only and
unindexed, so:

- You can't search across all your past sessions.
- You can't replay the moment you fixed a tricky bug.
- You can't tell whether you've already hit a particular error before.

Memex turns that pile of JSONL into a queryable, replayable, time-machine UI
backed entirely by **local** Qdrant + FastEmbed inference. Nothing leaves your
machine.

---

## Five Qdrant-unique features (the moat)

Most "session search" tools index everything into one vector and call it a
day. Memex puts five *different lenses* on the same point and lets you weight
them independently. Each lens uses a Qdrant primitive that you'd be hard
pressed to build on anything else:

| # | Feature | Qdrant primitive used | What you see in the UI |
|---|---|---|---|
| 1 | **Lens slider** | Multiple **named vectors** per point + parallel `query()` calls + weighted score combine in Rust | 5 sliders (`content` / `tool` / `path` / `error` / `code`) — slide to bias toward a signal; results show per-vector contribution chips |
| 2 | **Mix & Match** | **Discovery API** (`DiscoverInput` with context pairs) | Pick 1+ sessions as positives, 1+ as negatives — Qdrant returns the sessions that lie closer to the positives and farther from the negatives |
| 3 | **Topology** | **Distance Matrix API** (`search_matrix_pairs`) + petgraph MST | A radial SVG of every session in your collection, with MST edges weighted by similarity |
| 4 | **Replay** | Payload `source_path` lookup + on-demand JSONL re-parse | Turn-by-turn animation of any past session, with Bash terminal, Edit diff, Read snippet, and Task spawn visualizations at 1×/2×/4×/8× |
| 5 | **Proactive recall** | Background poller + `query()` on the `error` named vector with `has_errors=true` filter | When your live session hits a new error, a banner slides in: *"I've seen this error before — open the past session that solved it"* |

ColBERT v2 sentence-level highlighting (Plan §3 T3.4) is queued for a future
iteration; `fastembed-rs` 5.13.4 doesn't ship Jina ColBERT v2 yet.

---

## Install + run

### Prerequisites

- macOS 11+ (Apple Silicon recommended; tested on macOS 26.5 / arm64)
- [Rust](https://rustup.rs) 1.88+
- [Node.js](https://nodejs.org) 22+ with npm
- [Qdrant](https://github.com/qdrant/qdrant/releases) 1.18+ as a local
  binary or via Docker

### Step 1 — Clone

```bash
gh repo clone sgwannabe/memex ~/memex
cd ~/memex
npm install
```

### Step 2 — Start Qdrant

Either download the prebuilt binary…

```bash
mkdir -p .qdrant && cd .qdrant
curl -sL https://github.com/qdrant/qdrant/releases/download/v1.18.0/qdrant-aarch64-apple-darwin.tar.gz | tar xz
./qdrant            # leaves a Qdrant on localhost:6333 + 6334
```

…or run it via Docker:

```bash
docker run -d -p 6333:6333 -p 6334:6334 qdrant/qdrant:v1.18.0
```

Either way, verify with `curl localhost:6333 | jq .title` — it should print
`"qdrant - vector search engine"`.

### Step 3 — Authorize access to `~/.claude/projects`

On macOS Sequoia / Tahoe, the first time the .app reads
`~/.claude/projects` you'll get a permission prompt for the parent app
(Terminal, iTerm, etc.). For a packaged Memex.app you need to grant
**Full Disk Access** in System Settings → Privacy & Security → Full Disk
Access. Memex never sends your sessions anywhere — all embedding +
similarity work happens locally in Rust + Qdrant.

### Step 4 — First index

The CLI is the same binary as the app; it just dispatches on the first arg.
A first-time run downloads the BGE-small-en-v1.5 ONNX model (~130 MB) into
`.fastembed_cache/`.

```bash
# Build once (release ~3 min, dev ~30 s)
cargo build --release --manifest-path src-tauri/Cargo.toml

# Walk ~/.claude/projects and bulk-index into Qdrant
./src-tauri/target/release/memex scan --index
```

You should see something like `parsed 80 session(s) (shown: 80), 17752 total
tool calls` followed by a progress bar that fills to `80/80`.

### Step 5 — Run the desktop app

```bash
npm run tauri dev      # hot reload, useful during dev
# OR
npm run tauri build    # produces src-tauri/target/release/bundle/macos/Memex.app
```

When the window opens the bottom status bar should say
`Connected — 80 sessions indexed (memex_sessions)`. Hit **⌘K**, type a
query, and watch the cards rank.

---

## CLI reference

```
memex scan [--index] [--path PATH] [--limit N]
memex search "query"                           # plain content-vector search
memex lens "query" --content 2 --tool 1.5 --code 0.5
memex mix --pos <session_id> --neg <session_id>
memex topology --sample 80 --per-point 4 --out topo.json
memex recall "Tauri build failed missing icons"
memex snapshot export ./memex.snapshot
memex snapshot import ./memex.snapshot
```

`memex --help` lists everything; each subcommand has its own `--help`.

---

## Architecture (one screen)

```
┌─────────────────────────────────────────────────────────┐
│  Memex.app (Tauri 2)                                    │
│  ┌───────────────────────────────────────────────────┐  │
│  │ Frontend (vanilla HTML/CSS/JS in webview)         │  │
│  │ — Lens sliders / search bar / topology / replay   │  │
│  │ — invoke('<command>', args)                       │  │
│  └────────────────┬──────────────────────────────────┘  │
│                   │ Tauri IPC                            │
│  ┌────────────────▼──────────────────────────────────┐  │
│  │ Rust core (src-tauri/src)                         │  │
│  │ — parser.rs   ~/.claude/projects/**/*.jsonl       │  │
│  │ — indexer.rs  fastembed BGE-small + qdrant-client │  │
│  │ — commands.rs Tauri command surface               │  │
│  └────────────────┬──────────────────────────────────┘  │
└───────────────────┼──────────────────────────────────────┘
                    │ gRPC / HTTP
┌───────────────────▼──────────────────────────────────────┐
│  Local Qdrant (binary or Docker, port 6333/6334)        │
│  Collection `memex_sessions`:                           │
│   point_id = uuid_v5(session_id)                        │
│   vectors  = { content, tool, path, error, code }       │
│             5 × 384-d cosine BGE-small                  │
│   payload  = { project_name, project_path, git_branch,  │
│                ai_title, start_iso, has_errors, ... }   │
└──────────────────────────────────────────────────────────┘
```

More detail: [docs/architecture.md](docs/architecture.md),
[docs/qdrant-features.md](docs/qdrant-features.md).

---

## Status

This is a **hackathon MVP** (built in ~12 days for VSD 2026). Quality bar:
the functional path works end-to-end on the author's `~/.claude/projects`
(80 sessions, 17,752 tool calls), with all five Qdrant features verifiable
via either the CLI or the desktop app.

Known deferrals that didn't make the cut:

- **ColBERT v2 inline citations** — `fastembed-rs` doesn't ship the model.
  Fallback path via `ort` crate is planned.
- **BM42 sparse** on the `path` vector — same upstream limitation.
- **`notify` file watcher** — replaced with a 12 s polling loop. Both code
  paths are in tree; can swap in one line.
- **Native file picker for snapshot** — currently uses `window.prompt()`.
  Will move to `tauri-plugin-dialog`.
- **Layered Time-Machine card stack UI** — the radial result list works;
  the layered stack from `src/demo.html` is a polish iteration.

---

## License

[Apache 2.0](LICENSE) © 2026 Sangguen Chang.

Builds on the excellent work of [Qdrant](https://github.com/qdrant/qdrant),
[Tauri](https://github.com/tauri-apps/tauri),
[fastembed-rs](https://github.com/Anush008/fastembed-rs), and
[petgraph](https://github.com/petgraph/petgraph).
