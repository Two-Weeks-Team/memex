<div align="center">

# Memex

### Your AI session history as a navigable spatial memory.

*Vannevar Bush imagined the original [Memex](https://en.wikipedia.org/wiki/Memex) in 1945 — a personal knowledge machine built on **associative trails**, not search boxes. Eighty years later this is its desktop reincarnation: five Qdrant primitives wired into one **non-chatbot** UI for moving through, replaying, and learning from every Claude Code session you've ever run.*

<p>
  <a href="https://github.com/Two-Weeks-Team/memex/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/Two-Weeks-Team/memex/actions/workflows/ci.yml/badge.svg"></a>
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache_2.0-blue.svg?style=flat-square"></a>
  <a href="https://tauri.app"><img alt="Tauri" src="https://img.shields.io/badge/Tauri-2.x-24c8db?style=flat-square&logo=tauri&logoColor=white"></a>
  <a href="https://qdrant.tech"><img alt="Qdrant" src="https://img.shields.io/badge/Qdrant-1.18-dc382d?style=flat-square&logo=qdrant&logoColor=white"></a>
  <a href="https://www.rust-lang.org"><img alt="Rust" src="https://img.shields.io/badge/Rust-1.88-orange?style=flat-square&logo=rust&logoColor=white"></a>
  <a href="#install--run"><img alt="macOS" src="https://img.shields.io/badge/macOS-11%2B_(Apple_Silicon)-000000?style=flat-square&logo=apple&logoColor=white"></a>
  <br>
  <img alt="100% local" src="https://img.shields.io/badge/privacy-100%25_local-30d158?style=flat-square">
  <img alt="No telemetry" src="https://img.shields.io/badge/telemetry-none-30d158?style=flat-square">
  <img alt="No LLM at runtime" src="https://img.shields.io/badge/LLM_at_runtime-none-30d158?style=flat-square">
  <img alt="Hackathon" src="https://img.shields.io/badge/Qdrant_VSD_2026-Think%20Outside%20the%20Bot-bf5af2?style=flat-square">
</p>

<p>
<a href="https://sgwannabe.github.io/memex/"><b>🌐 Landing page</b></a> ·
<a href="#-seven-surfaces-zero-chat-windows"><b>Surfaces</b></a> ·
<a href="#-what-you-can-do-with-memex"><b>Use cases</b></a> ·
<a href="#-quick-start"><b>Quick start</b></a> ·
<a href="#-cli-reference"><b>CLI</b></a> ·
<a href="#-architecture"><b>Architecture</b></a> ·
<a href="#-status--roadmap"><b>Status</b></a>
</p>

</div>

---

## 📌 At a glance

| | |
|---|---|
| **Repo (canonical)** | [`Two-Weeks-Team/memex`](https://github.com/Two-Weeks-Team/memex) |
| **Team / author** | Two-Weeks-Team · Sangguen Chang |
| **Release** | [**v0.1.0**](https://github.com/Two-Weeks-Team/memex/releases/tag/v0.1.0) — `Memex_0.1.0_aarch64.dmg`, macOS Apple Silicon, **unsigned** ([install notes](docs/INSTALL.md)) |
| **License** | [Apache-2.0](LICENSE) |
| **Built for** | [Qdrant Vector Space Day 2026](https://qdrant.tech) — *"Think Outside the Bot."* All code authored during the hackathon build period (May 2026). |
| **Qdrant role** | **Load-bearing, not a sidecar** — five distinct Qdrant primitives *are* the product. **No chatbot, no LLM at runtime.** |
| **Landing page** | [sgwannabe.github.io/memex](https://sgwannabe.github.io/memex/) (static single-file, no JS) |

### 🧑‍⚖️ Judge path in 5 steps

```bash
# 1. Start Qdrant (load-bearing dependency) — one command
bash scripts/start-qdrant.sh

# 2. Install OR source-build Memex
#    install : https://github.com/Two-Weeks-Team/memex/releases/latest   (Gatekeeper steps → docs/INSTALL.md)
#    source  : cargo build --release --manifest-path src-tauri/Cargo.toml  (platforms → docs/BUILD.md)

# 3. Load the sample corpus (synthetic — no private data)
./src-tauri/target/release/memex scan --path examples/sample-corpus --index

# 4. Open the spatial memory view (GUI)   → npm run tauri build && open .../Memex.app
#    …or stay in the terminal for step 5.

# 5. Inspect the Qdrant-backed features
memex lens     "build error" --error 2.0 --content 1.0    # named-vector lens
memex mix      --pos <id> --neg <id>                       # Discovery API
memex topology --sample 12 --out /tmp/topo.json            # Distance Matrix → MST
memex recall   "cargo build linker error"                  # proactive recall
#    replay = GUI surface, played over the indexed sessions
```

Proof it all runs end-to-end: **[docs/e2e-evidence.md](docs/e2e-evidence.md)** ·
CLI examples + expected hits: **[examples/sample-corpus/README.md](examples/sample-corpus/README.md)**

---

## 🗄️ Why Memex exists — your AI memory survives Anthropic's silent migrations

Claude Code rewrites its own session storage every few months **without
announcing it** and ships auto-updates that silently delete the old files.
On a typical user's machine right now:

| | path | files |
|---|---|---|
| **Legacy** (pre-v2.1.114, ~Apr 2026) | `~/.claude/transcripts/ses_*.jsonl` | thousands of older sessions, no longer written to |
| **Modern** | `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl` | last 30-ish days only |
| **Prompt history** (survives migrations) | `~/.claude/history.jsonl` | every prompt you ever typed |

Anthropic announced **none** of this. Search the official
[CHANGELOG.md](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md)
for "transcripts directory" or "migration" and you get zero hits. Meanwhile
GitHub is full of OPEN data-loss reports —
[#41591](https://github.com/anthropics/claude-code/issues/41591) (520 sessions
silently deleted by 2.1.87 auto-update),
[#54907](https://github.com/anthropics/claude-code/issues/54907) (all sessions
lost across the 2.1.114 → 2.1.123 upgrade),
[#48782](https://github.com/anthropics/claude-code/issues/48782) (160 jsonls
× 60 702 messages gone),
[#41458](https://github.com/anthropics/claude-code/issues/41458)
(`cleanupPeriodDays: 99999` ignored, 490 sessions deleted anyway),
[#23710](https://github.com/anthropics/claude-code/issues/23710),
[#59248](https://github.com/anthropics/claude-code/issues/59248), …

**What Memex does about it:**

1. **Reads both legacy and modern jsonl paths** — `parser::parse_transcript_session`
   handles the older `{type, timestamp, content}` schema, so your last
   1 000–2 000 transcripts join the modern corpus on the same Qdrant point space.
2. **Uses `~/.claude/history.jsonl` as the timeline base layer** —
   24 000+ prompts across 6–12 months survive every Claude Code migration.
   The dashboard's heatmap is drawn from this, with indexed sessions overlaid.
3. **One-click Qdrant snapshot** — once you've indexed, your corpus is
   yours. Future Anthropic cleanups can't touch the points sitting in
   `qdrant_storage/`.

Memex's reason to exist isn't "vector search on top of Claude Code". It's
**"vector search on top of a corpus you actually own — preserved against
Anthropic's silent migrations."**

---

## 🛑 Why Memex isn't a chatbot

Qdrant Vector Space Day 2026's prompt is unusually direct:

> **"Think Outside the Bot."** *"Forget the classical RAG chatbot."*
> Reimagine vector search beyond conversational interfaces — multi-modal apps, intelligent recommendations, advanced vector search.

Memex takes that literally. There is **no chat window**, **no LLM call at runtime**, **no "ask a question" affordance**. Instead it treats your `~/.claude/projects/**/*.jsonl` corpus the way Bush imagined his Memex would treat a researcher's library: as a **spatial memory** you can step into, point at, and traverse by *similarity* rather than by keyword.

Concretely:

| The "obvious" RAG chatbot version of this | What Memex does instead |
|---|---|
| A text box asking "what session am I looking for?" | A **3D card stack** (Time Machine) showing every past session, navigated by ↑↓ / wheel. |
| Embed-and-retrieve a session's text, summarize it with an LLM. | **Replay** the session turn-by-turn in the *original* webview surface — Bash terminals, Edit diffs, Read snippets, exactly as you saw them live. |
| Answer "have I seen this error before?" via RAG → LLM → text. | **Banner slides in** with the past session whose `error` named-vector neighborhood matches — zero LLM calls. |
| "What other sessions are like this one?" → LLM compares summaries. | **Mix & Match** drops session points into Qdrant's Discovery API and returns ranked neighbors. |
| "What's the structure of my work?" → LLM writes a paragraph. | **3D force-directed topology** of `search_matrix_pairs` data, with auto-labeled clusters, cross-project bridge edges, and gap insights ("‘project-redesign’ ↔ ‘project-yc’ have semantically similar sessions but no bridge — possible unmade connection."). |
| "What should I do next?" → LLM completion + tool-use. | **🔮 Predict next-action** — embed the active session's last few turns, find K similar past sessions via the `content` vector, locate the conversational pivot, walk forward `horizon` turns, aggregate tool calls. Surfaces what *past-you did* from a comparable position. Zero LLM. |

Six different surfaces, multiple Qdrant primitives, zero generative AI in the loop.

---

## 🧠 The corpus

Every Claude Code session you've ever run is sitting on your laptop right now:

```
~/.claude/projects/<encoded-cwd>/<session-uuid>.jsonl
```

Inside each `.jsonl` is your *entire* conversation — every prompt, every tool call, every diff, every output, every error. **Months of personal engineering memory, perfectly preserved, but practically unreachable** without a tool like this.

| Without Memex | With Memex |
|---|---|
| 📁 You have N "social-seeding-v2/v3/v4" projects — were they actually different work, or did you redo it? | Topology cluster auto-labels: *"project-marketing (10 sess) — code + shell · Bash×1350 Edit×1032"*. Three v#'s collapse into one bubble. |
| 🔁 You hit the same `WAL Kind(WouldBlock)` you already debugged last month. | A banner slides in: *"I've seen this — open the session that solved it."* (No LLM, no chat — just a named-vector neighbor.) |
| ⏯ You want to *re-watch* yourself fix a tricky bug. | Open the session in Replay. Step through 600 turns at 4×, see every Bash output and Edit diff exactly as it happened. |
| 🌌 "What did I work on last month?" | A 3D galaxy of every session, color-coded by project, with yellow cross-project bridges where ideas jumped — and gap cards flagging missed connections. |
| 🌐 You stitch results from cloud-hosted, telemetry-bearing services. | Parsing, embedding, similarity search, replay — **all on your machine**. Zero network calls after `cargo build`. |

> Memex turns your `.jsonl` pile into a **spatial, replayable memory machine** powered entirely by local Qdrant + FastEmbed.

---

## 🪟 Demo

Real screenshots from the running app (macOS · Apple Silicon) over an indexed corpus of **258 Claude Code + Codex sessions**.

| 🪟 Time Machine stack | 🌌 Topology galaxy | 🧪 Mix & Match |
|:---:|:---:|:---:|
| _Layered 3D card deck of every past session — ↑↓ / wheel to time-travel._ | _Force-directed galaxy: project clusters, cross-project bridges, gap insights._ | _Discovery hyperplane — drop positives & negatives, get recommendations._ |
| ![Time Machine layered card stack](docs/img/stack.png) | ![3D topology galaxy with project cluster labels](docs/img/topology.png) | ![Mix & Match discovery hyperplane](docs/img/mix-match.png) |
| 🔍 **Lens search** | 🔮 **Predict next-action** | ⏯ **Replay engine** |
| _Weighted multi-named-vector search with per-vector contribution chips._ | _"What past-you did next" — neighbor pivot-walk + tool-call ranking._ | _Turn-by-turn playback with Bash / Edit-diff / Read / Task views._ |
| ![Lens weighted multi-vector search results](docs/img/lens.png) | ![Predict next-action panel](docs/img/predict.png) | ![Replay engine turn-by-turn playback](docs/img/replay.png) |

> 📹 A walkthrough video is **not part of this hackathon submission** — the six surfaces above are captured live from the app.

---

## ⬇️ Download

**[→ Download Memex v0.1.0 for macOS (Apple Silicon)](https://github.com/Two-Weeks-Team/memex/releases/latest)** — `Memex_0.1.0_aarch64.dmg` (~16 MB)

> ⚠️ **First launch (Gatekeeper):** Memex is an **unsigned MVP** — ad-hoc signed, with no Apple notarization. macOS will refuse a normal double-click the first time. Open the `.dmg`, drag **Memex.app** to `/Applications`, then **right-click → Open** and confirm in the dialog — or clear the quarantine flag with `xattr -dr com.apple.quarantine /Applications/Memex.app`. You only need to do this once. **Full clean-machine steps + source-build fallback: [docs/INSTALL.md](docs/INSTALL.md).**

Memex also needs a local **Qdrant** on `localhost:6334` (it self-heals if you start Qdrant after launch) — see [Quick start](#-quick-start). Prefer to compile it yourself? [Build from source](#-quick-start).

---

## ✨ Seven surfaces, zero chat windows

Each surface in Memex maps to a different Qdrant primitive — together they cover *named vectors → matrix sampling → discovery → payload filtering → snapshots → recommendation*. None of these are the "embed text, retrieve top-K, feed to LLM" loop of classical RAG.

Ordered as you encounter them in the app (visual / spatial first, search last):

| # | Surface | Qdrant primitive | What you actually do |
|---|---|---|---|
| 1 | 🪟 **Time Machine layered stack** | `scroll` over the indexed collection (payload-only, no vectors) | When the app boots, every past session appears as a 3D layered card deck. ↑↓ / mouse-wheel time-travels through them. **No search box involved.** |
| 2 | 🌌 **Topology galaxy** | **Distance Matrix API** (`search_matrix_pairs`) → 3D force-directed graph + auto-clustered project labels + gap insights | A WebGL scene of your session corpus. Cluster auto-labels (*"code + shell · Bash×1350 Edit×1032"*), yellow cross-project bridge edges, and **Gap cards** flagging pairs of projects that *should* connect but don't (*"‘project-redesign’ ↔ ‘project-yc’ — semantically similar (sim 0.97) but never bridged."*). |
| 3 | 🧪 **Mix & Match** | **Discovery API** (`DiscoverInput` + context pairs) | Drop sessions as **positives** and **negatives** — Qdrant returns sessions semantically near the positives, far from the negatives. Recommendation, not retrieval. |
| 4 | 🔔 **Proactive recall** | `query()` on the dedicated `error` named vector with `has_errors=true` payload filter, polled every 12 s over `~/.claude/projects` | Working in another Claude Code session and hit a fresh `tool_result.is_error`? A banner slides in: *"I've seen this error before — open the session that solved it."* No LLM, no chat, just a vector neighbor with the right filter. |
| 5 | 🔮 **Predict next-action** <kbd>NEW</kbd> | `content` named-vector neighbor search + payload re-parse + tool-call aggregation | Click a session — Memex embeds its last 3 turns, finds 8 similar past sessions, lexically locates the *pivot turn* in each, walks `horizon` turns forward, and ranks the tool calls by `frequency × similarity`. The panel surfaces "what past-you did next" with a one-click jump-to-replay back at the source turn. **The recommendation answer to "what should I do?" without an LLM in sight.** |
| 6 | ⏯ **Replay engine** | Lightweight payload (`source_path`) → on-demand JSONL re-parse | Turn-by-turn animation of any past session with **Bash terminals**, **Edit `-`/`+` diffs**, **Read snippets**, **Task/Agent spawns**. Click to scrub, ⏮ ⏯ ⏭ controls, 1× / 2× / 4× / 8×. (No vector primitive here — but it's the surface Memex's vector primitives *point to*.) |
| 7 | 🔍 **Lens slider** | Multiple **named vectors per point** + parallel `query()` + weighted Rust combine | The "advanced vector search" axis, intentionally last. Five named vectors per session (`content`, `tool`, `path`, `error`, `code`); slide each weight to bias the rank — per-vector contribution chips on each result card so you can *see* which lens earned the hit. |

Plus: **📦 Snapshot** export/import via Qdrant's HTTP snapshot API — your entire indexed memory in one portable file.

ColBERT v2 inline citations are on the roadmap; [`fastembed-rs`](https://github.com/Anush008/fastembed-rs) 5.x doesn't yet ship the model.

---

## 🧠 MCP integration — Memex as a memory layer for any AI agent

Memex ships its own [**Model Context Protocol**](https://modelcontextprotocol.io) server (stdio JSON-RPC, hand-rolled, zero external runtime). Once you `claude mcp add memex …` once, every Claude Code session — and any other MCP-aware client (Codex, Cursor, …) — can call into your local session corpus mid-conversation. No new network calls, no third-party SaaS.

```bash
# one-time wiring — point Claude Code at the same memex binary you already run
claude mcp add memex /path/to/memex/src-tauri/target/release/memex mcp

claude mcp list
#  memex: /…/memex mcp - ✓ Connected
```

…or print the exact command for your machine:

```bash
memex install-mcp           # echoes the `claude mcp add …` line
memex install-mcp --run     # actually runs it
```

The server exposes **9 tools** mapping directly to the same Qdrant primitives that power the desktop UI:

| Tool | What it does |
|---|---|
| `find_similar_sessions(query, limit?, weights?)` | Five-vector Lens search over your past sessions. Per-vector contribution scores in the response. |
| `find_similar_error(error_text, limit?)` | Targeted neighbor search on the `error` named vector, filtered to `has_errors=true`. Returns sessions that *also* hit a similar error — typically the ones that resolved it. |
| `predict_next_action(session_id, last_n_turns?, horizon?, neighbors?)` | "What would past-you do next?" — neighbor walk + tool-call aggregation, returns ranked `(tool, example_input, source_session, turn_index)` with `frequency × similarity`. |
| `mix_similar_sessions(positive[], negative[], limit?)` | Qdrant Discovery API — sessions near the positives, away from the negatives. |
| `get_session_summary(session_id)` | Metadata payload: project, branch, ai_title, start/end, turn counts, has_errors. |
| `get_session_turn(session_id, turn_index)` | A single turn, re-parsed from source jsonl — full text + tool calls + tool results. |
| `list_recent_sessions(limit?)` | Most-recent-first walk of `~/.claude/projects` — works even before Qdrant is fully warm. |
| `analyze_corpus_topology(sample?, per_point?)` | MST of session content vectors, per-project auto-labels, cross-project bridges, and gap insights. |
| `snapshot_export(path)` | Server-side snapshot of the entire collection to a portable `.snapshot` file. |

Example transcript inside a Claude Code session, with Memex wired up:

```text
> I'm hitting the same WAL Kind(WouldBlock) again. Have I dealt with this before?

⏺ memex - find_similar_error (MCP)
  ⎿  3 past sessions found:
       1. project-redesign · 2026-04-12 · sim 0.91 · "fix wal contention in indexer"
       2. memex          · 2026-03-30 · sim 0.84 · "Phase 6 polling + recall"
       3. ckm-rails      · 2026-02-04 · sim 0.71 · "concurrent migration retries"

⏺ memex - get_session_turn { session_id: "…redesign…", turn_index: 487 }
  ⎿  …shows the exact fix you applied last time…

> Nice. Apply the same fix here.
```

Behind the scenes Memex stays 100 % local — no LLM calls inside the server, no telemetry. The MCP surface is a *typed handle* on the Qdrant index your desktop app is already using; the daemon never speaks to anything outside `localhost:6334`.

> **Auto-index daemon + macOS notifications:** while the app is open, a 60 s
> background watcher catches any new session jsonl, embeds it, and upserts it
> into Qdrant. If a fresh `tool_result.is_error` matches a *different* past
> session above the 0.65 similarity threshold, a macOS notification pops:
> *"Memex · I've seen this error before · &lt;project&gt; · turn #N"*.
> Clicking it brings the app to focus and auto-opens the past session's
> replay so you can scrub through how you fixed it last time.

---

## 💡 What you can *do* with Memex

Not "what you can ask" — there's no question-answering interface. These are spatial, temporal, and recommendation moves you make on your own corpus:

<table>
<tr><td><b>Browse your work, no query needed</b></td><td>

Launch the app. The Time Machine stack populates with every past session sorted most-recent first. **No search box involved.**

```
↑ / ↓     time-travel through 80 past sessions
⏎         open the focused session in the inspector
mouse-wheel  smooth scrolling through history
```

</td></tr>
<tr><td><b>See the shape of your work</b></td><td>

Open the Topology galaxy. Same-project sessions form clusters; yellow lines are cross-project "bridges" (= shared ideas).

```
→ "project-marketing (10 sess) — code + shell · Bash×1350 Edit×1032"
→ Gap card: "project-redesign ↔ project-yc — semantically similar
            (sim 0.97) but never bridged"
```

The Gap insights are an *intelligent recommendation*, not a search result: they tell you about connections you've *never* made between your own projects.

</td></tr>
<tr><td><b>Recommend, don't retrieve</b></td><td>

Mix & Match drops session points into Qdrant's Discovery API. Two clicks → ranked recommendations.

```
+ pos:  workspace-a session
− neg:  project-meeting session
→ Discover: workspace-b, workspace-c, project-redesign …
   "Sessions like the panel-flavored work, unlike chatty meetings."
```

</td></tr>
<tr><td><b>Get reminded automatically</b></td><td>

A background poller watches `~/.claude/projects` for `tool_result.is_error`. When a fresh one appears, a banner slides in within 12 s:

```
⚡ I've seen this error before:
   project-redesign — 2026-05-15 (sim 0.93)
   [Open replay]   [Dismiss]
```

(No LLM call. No chat surface. Just a Qdrant `query()` against the `error` named vector with `has_errors=true` filter.)

</td></tr>
<tr><td><b>🔮 See what past-you did next</b></td><td>

Click any session in the stack. The inspector's prediction panel populates within ~1 s:

```
🔮 What past-you did next         2 of 6 neighbor(s) matched

#1  🖥 Bash    67% of times · sim 65%
     cargo build --release
     ● project-redesign · turn #486    [Jump to replay]

#2  ✏️ Edit    33% of times · sim 64%
     src-tauri/Cargo.toml
     ● project-tool-a · turn #312      [Jump to replay]
```

The closest thing Memex has to "what should I do next?" — answered purely by neighbor-vector lookup + tool-call aggregation. The Jump-to-replay button warps you to the exact source turn so you can see the resolution play out.

</td></tr>
<tr><td><b>Re-experience a past session</b></td><td>

Click Replay on any card. The Replay engine animates the session turn-by-turn at 1× / 2× / 4× / 8× — Bash terminals, Edit `-`/`+` diffs, Read snippets, Task/Agent spawns, every tool exactly as the user saw it live.

```
600 turns at 4×  ≈ 5 min replay
```

</td></tr>
<tr><td><b>Search, if you still want to</b></td><td>

⌘K opens the Lens. Slide each named vector weight to bias the rank toward `content`, `tool`, `path`, `error`, or `code` — per-vector contribution chips on each card so you can see which lens earned the hit.

```
memex lens "Tauri build failed missing icons" --error 2 --tool 1
```

The Lens slider is intentionally the *last* surface, not the first.

</td></tr>
</table>

</td></tr>
</table>

---

## 🚀 Quick start

```bash
# 1. Clone + install JS deps
gh repo clone Two-Weeks-Team/memex ~/memex && cd ~/memex && npm install

# 2. Start Qdrant (binary path — or docker run -d -p 6333:6333 -p 6334:6334 qdrant/qdrant:v1.18.1)
mkdir -p .qdrant && curl -sL https://github.com/qdrant/qdrant/releases/download/v1.18.1/qdrant-aarch64-apple-darwin.tar.gz | tar xz -C .qdrant
./.qdrant/qdrant &

# 3. Index your ~/.claude/projects (downloads BGE-small ~130 MB on first run)
cargo build --release --manifest-path src-tauri/Cargo.toml
./src-tauri/target/release/memex scan --index

# 4. Launch the app
npm run tauri build   # produces src-tauri/target/release/bundle/macos/Memex.app
open src-tauri/target/release/bundle/macos/Memex.app
```

That's it. Hit **⌘K**, type something you worked on last month, watch the cards rank.

<details>
<summary><b>📋 Full prerequisites + step-by-step (click to expand)</b></summary>

### Prerequisites

- **macOS 11+** (Apple Silicon recommended; tested on macOS 26.5 / arm64)
- [**Rust**](https://rustup.rs) 1.88+
- [**Node.js**](https://nodejs.org) 22+ with npm
- [**Qdrant**](https://github.com/qdrant/qdrant/releases) 1.18+ (binary or Docker)

### Step 1 — Clone

```bash
gh repo clone Two-Weeks-Team/memex ~/memex
cd ~/memex
npm install
```

### Step 2 — Start Qdrant

**One command** (recommended — uses the pinned `docker-compose.yml`):

```bash
bash scripts/start-qdrant.sh
```

This starts `qdrant/qdrant:v1.18.1`, waits for `/readyz`, and prints the
health-check command. Qdrant listens on:

| Port | Protocol | Used by | URL |
|---|---|---|---|
| **6334** | gRPC | **Memex** (`MEMEX_QDRANT_URL`, default `http://localhost:6334`) | `http://localhost:6334` |
| 6333 | REST + dashboard + health | health checks, web UI | `http://localhost:6333/dashboard` |

Health check: `curl -fsS http://localhost:6333/readyz && echo OK`.
Stop later with `bash scripts/start-qdrant.sh --stop` (data is preserved in the `qdrant_storage` volume).

<details>
<summary>Alternatives (raw Docker, or prebuilt binary)</summary>

```bash
# Raw docker (no compose):
docker run -d --name memex-qdrant -p 6333:6333 -p 6334:6334 qdrant/qdrant:v1.18.1

# Prebuilt binary (no Docker — Apple Silicon shown):
mkdir -p .qdrant && cd .qdrant
curl -sL https://github.com/qdrant/qdrant/releases/download/v1.18.1/qdrant-aarch64-apple-darwin.tar.gz | tar xz
./qdrant            # serves Qdrant on localhost:6333 (HTTP) + 6334 (gRPC)
```

</details>

Verify: `curl localhost:6333 | jq .title` should print `"qdrant - vector search engine"`.

### Step 3 — Authorize Full Disk Access

On **macOS Sequoia / Tahoe**, granting `Memex.app` **Full Disk Access** in System Settings → Privacy & Security is required so it can read `~/.claude/projects`. Memex never sends your sessions anywhere — every embedding and similarity call happens locally in Rust + Qdrant.

### Step 4 — First index

The CLI is the same binary as the GUI; it dispatches on `argv[1]`. The first run downloads the BGE-small-en-v1.5 ONNX model (~130 MB) into `.fastembed_cache/`.

```bash
cargo build --release --manifest-path src-tauri/Cargo.toml
./src-tauri/target/release/memex scan --index
```

You should see:
```
parsed 80 session(s) (shown: 80), 17752 total tool calls
indexed 79/80 session(s) into 'memex_sessions' (1 duplicate sessionId(s) skipped, 0 error(s))
```

### Step 5 — Launch

```bash
npm run tauri dev      # hot-reload dev mode (WebKit Inspector ON)
# OR
npm run tauri build    # local release build (WebKit Inspector ON)
# OR — distribution build (.dmg you'd hand to someone else)
npm run tauri:dist     # → bundle/dmg/Memex_*.dmg, Inspector OFF for shipping
```

`tauri:dist` passes `--no-default-features`, which drops the `devtools`
Cargo feature so the resulting bundle is the production-hygiene one
(no right-click → Inspect Element on the shipped `.dmg`). `npm run
tauri dev` and `npm run tauri build` keep Inspector on so local
debugging isn't blocked. See the `[features]` note in
[`src-tauri/Cargo.toml`](src-tauri/Cargo.toml) for the devtools threat-model
rationale.

When the window opens, the bottom status bar should read:
```
Connected — 79 sessions indexed (memex_sessions)
```

</details>

---

## 🛠 CLI reference

Memex's CLI is a one-binary surface over the same backend the GUI uses:

```bash
memex scan [--index] [--path PATH] [--limit N]    # walk + (optionally) index
memex search "query"                              # plain content-vector search
memex lens "query" --content 2 --tool 1.5 --code 0.5
memex mix --pos <session_id> --neg <session_id>
memex topology --sample 80 --per-point 6 --out topo.json
memex recall "Tauri build failed missing icons"
memex predict <session_id> --last-n 3 --horizon 3 --neighbors 8
memex snapshot export ./memex.snapshot
memex snapshot import ./memex.snapshot
```

Run `memex --help` for the full surface; each subcommand has `--help` too.

---

## 🏗 Architecture

```mermaid
flowchart TB
    subgraph fs["~/.claude/projects (your laptop)"]
        jsonl["<session-uuid>.jsonl<br>append-only"]
    end

    subgraph app["Memex.app · Tauri 2"]
        webview["Webview (HTML/CSS/JS)<br>Time Machine stack · 3D topology · replay · banner"]
        rustcore["Rust core<br>parser.rs · indexer.rs<br>commands.rs · cli.rs"]
        webview <-- "Tauri IPC<br>invoke('lens_search', …)" --> rustcore
    end

    subgraph qdrant["Local Qdrant 1.18"]
        coll["Collection memex_sessions<br>5 named vectors / point (384-d cosine)<br>payload-indexed: project_name, start_ts, has_errors, …"]
    end

    fs -- walkdir + serde_json --> rustcore
    rustcore -- "fastembed BGE-small<br>+ qdrant-client gRPC" --> coll
    rustcore -. "reqwest HTTP<br>(snapshots only)" .-> coll
```

Each session becomes **one point** with **five named vectors** (`content`, `tool`, `path`, `error`, `code`) all dense 384-d BGE-small. The payload carries only metadata — replay re-parses the JSONL on demand so Qdrant stays lean.

Deeper reading:
- [`docs/architecture.md`](docs/architecture.md) — data flow, schema, design trade-offs
- [`docs/qdrant-features.md`](docs/qdrant-features.md) — engineer's tour of each of the 5 features
- [`docs/memex/PLAN.md`](docs/memex/PLAN.md) — original 8-phase implementation plan

---

## 🔬 Tech stack

<table>
<tr>
<td><b>Frontend</b></td>
<td>

`vanilla HTML/CSS/JS` · `Tauri 2 webview` · [`3d-force-graph`](https://github.com/vasturiano/3d-force-graph) (Three.js) for topology · CSS 3D `translateZ` for the Time Machine layered stack

</td>
</tr>
<tr>
<td><b>Backend</b></td>
<td>

`Rust 1.88` · [`tauri 2`](https://tauri.app) · [`qdrant-client 1.18`](https://github.com/qdrant/rust-client) · [`fastembed 5`](https://github.com/Anush008/fastembed-rs) (BGE-small-en-v1.5) · [`petgraph 0.6`](https://github.com/petgraph/petgraph) for MST · [`tokio`](https://tokio.rs) · `walkdir` · `serde` · `regex`

</td>
</tr>
<tr>
<td><b>Storage</b></td>
<td>

[`Qdrant 1.18`](https://qdrant.tech) (local binary or Docker) — 5 named dense vectors per point (384-d cosine), payload-indexed on `project_name`, `git_branch`, `start_ts`, `has_errors`, etc.

</td>
</tr>
<tr>
<td><b>Embedding</b></td>
<td>

`fastembed-rs` running BGE-small-en-v1.5 entirely client-side. No Python sidecar, no network calls, ~130 MB ONNX model cached after first run.

</td>
</tr>
<tr>
<td><b>Bundle</b></td>
<td>

`Memex.app` ~47 MB · `Memex_0.1.0_aarch64.dmg` ~16 MB, [shipped on the v0.1.0 release](https://github.com/Two-Weeks-Team/memex/releases/latest) · No code signing in MVP (ad-hoc only) — right-click → Open the first time.

</td>
</tr>
</table>

---

## 📊 Status & roadmap

This is a **hackathon MVP** built for [Qdrant Vector Space Day 2026](https://qdrant.tech) (deadline 2026-06-01). Verified end-to-end on the author's `~/.claude/projects` (**79 sessions indexed, 17,938 tool calls covered**), with all five primitives exercisable from both CLI and GUI.

**Hackathon alignment** — *"Think Outside the Bot"*:

- ✅ No chat surface · no LLM in the runtime loop · no "ask a question" affordance
- ✅ **5 distinct Qdrant primitives** (named vectors / Distance Matrix / Discovery / payload filter / Snapshot), each wrapped in a visual UI rather than a text retrieval pipeline
- ✅ Two of the surfaces (Proactive Recall, Mix & Match) are *recommendation* features — explicitly called out as an encouraged direction in the VSD prompt
- ✅ Single-machine, zero-telemetry, zero-network architecture

**What ships in this MVP**

- ✅ 🪟 Time Machine layered 3D card stack on boot (browse, no query needed)
- ✅ 🌌 3D force-directed topology galaxy with project cluster auto-labels + gap insights
- ✅ 🧪 Mix & Match recommendation via Qdrant Discovery API
- ✅ 🔔 Proactive recall banner (12 s poll over `~/.claude/projects`)
- ✅ 🔮 **Predict next-action** — neighbor-vector pivot walk + tool-call aggregation
- ✅ ⏯ Replay engine with Bash / Edit-diff / Read / Task tool visualizations at 1×–8×
- ✅ 🔍 Lens slider (multi-named-vector weighted search) — the "advanced vector search" axis
- ✅ 📦 Snapshot export/import via Qdrant HTTP API
- ✅ 🌐 Public landing page at [sgwannabe.github.io/memex](https://sgwannabe.github.io/memex/) (single-file `index.html`, no JS)
- ✅ Lazy AppState init — self-heals if Qdrant is started after Memex
- ✅ EROFS fix — fastembed cache + working-dir-on-launch for the bundled `.app`
- ✅ Honest duplicate-sessionId detection in indexer reporting
- ✅ `Memex.app` + `.dmg` for macOS arm64 — [downloadable on the v0.1.0 release](https://github.com/Two-Weeks-Team/memex/releases/latest)

**Deferred to post-MVP**

| Item | Why it's deferred | Path forward |
|---|---|---|
| ColBERT v2 inline citations | `fastembed-rs` doesn't yet expose the model | Fallback via `ort` crate + ONNX Jina-ColBERT-v2 |
| BM42 sparse on `path` vector | Same upstream gap | Same path |
| Real `notify` file watcher | Polling works and avoids fd-leak / macOS permission edge cases | Code path already in `Cargo.toml` — one-line swap when needed |
| Native file picker for snapshots | MVP uses `window.prompt()` | Add `tauri-plugin-dialog` |
| Code signing / notarization | Local-only MVP | Apple Developer cert when shipping publicly |

---

## 🤝 Contributing / feedback

This is a personal hackathon project, but PRs that don't break the demo are welcome — especially:
- Linux + Windows packaging
- Codex / Cursor / other CLI session formats (parser extension)
- ColBERT v2 integration via `ort`

For bugs or design feedback, [open an issue](https://github.com/Two-Weeks-Team/memex/issues/new).

---

## 📄 License

[Apache 2.0](LICENSE) © 2026 Sangguen Chang.

Built on the excellent open work of [Qdrant](https://github.com/qdrant/qdrant), [Tauri](https://github.com/tauri-apps/tauri), [fastembed-rs](https://github.com/Anush008/fastembed-rs), [petgraph](https://github.com/petgraph/petgraph), and [3d-force-graph](https://github.com/vasturiano/3d-force-graph).

<div align="center">
<sub>Made for <a href="https://qdrant.tech">Qdrant Vector Space Day 2026</a> · <a href="https://github.com/Two-Weeks-Team/memex">Two-Weeks-Team/memex</a></sub>
</div>
