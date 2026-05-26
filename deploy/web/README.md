# Memex — server variant (single Docker image)

A **server-based** Memex that runs **Qdrant + the web service + MCP in one
container** — no macOS app required. It's **additive**: the Tauri desktop app is
untouched; this is a second way to run Memex, aimed at the Claude CLI and
browsers.

| Surface | Where | Port |
|---|---|---|
| Web UI (static) | `/` | **8765** |
| JSON API | `/api/*` | 8765 |
| MCP over HTTP | `/mcp` (POST JSON-RPC, GET SSE) | 8765 |
| Qdrant (inside the image) | REST `/readyz` + dashboard / gRPC | 6333 / 6334 |

## Build & run (one image)

```bash
# from the repo root
docker build -t memex-allinone -f deploy/web/Dockerfile .
docker run --rm --name memex-allinone -p 8765:8765 -p 6333:6333 memex-allinone
```

That single container starts Qdrant internally, waits for `/readyz`, then serves
the web service. Persist the indexed corpus by mounting a volume at
`/qdrant/storage`:

```bash
docker run --rm --name memex-allinone -p 8765:8765 -v memex_qdrant:/qdrant/storage memex-allinone
```

## Load the sample corpus

The image ships the synthetic corpus at `/app/sample-corpus`:

```bash
curl -fsS -X POST http://localhost:8765/api/index \
  -H 'Content-Type: application/json' -d '{"path":"/app/sample-corpus"}'
# → {"indexed":12,"total":12,"errors":0,...}
```

> The embedding model is **pre-baked into the image** (no first-query network
> needed), and the container **auto-indexes the bundled corpus on startup**
> (`MEMEX_WEB_AUTOINDEX=1`) — so the browser UI shows data immediately. The
> manual `/api/index` call above is only for (re)indexing a different corpus.
> Point at your own sessions by mounting them under
> `/home/memex/.claude/projects` and setting `MEMEX_SCAN_ROOT`.

## JSON API

```bash
curl -fsS http://localhost:8765/api/health
curl -fsS "http://localhost:8765/api/search?q=rate%20limiter%20redis&limit=3"
curl -fsS "http://localhost:8765/api/recall?q=cargo%20build%20linker%20error"
curl -fsS "http://localhost:8765/api/topology?sample=12&per_point=4"
curl -fsS -X POST http://localhost:8765/api/lens -H 'Content-Type: application/json' \
  -d '{"query":"build error","weights":{"error":2.0,"content":1.0}}'
curl -fsS -X POST http://localhost:8765/api/mix -H 'Content-Type: application/json' \
  -d '{"pos":["<session_id>"],"neg":["<session_id>"]}'
```

## Register with the Claude CLI (both transports)

**HTTP (remote MCP — recommended for the web service):**
```bash
claude mcp add --transport http memex-web http://localhost:8765/mcp
```

**stdio (the same binary, inside the running container):**
```bash
claude mcp add memex-web -- docker exec -i memex-allinone memex mcp
```

Both expose the same 9 tools (`find_similar_sessions`, `find_similar_error`,
`predict_next_action`, `mix_similar_sessions`, `get_session_summary`,
`get_session_turn`, `list_recent_sessions`, `analyze_corpus_topology`,
`snapshot_export`).

## Local dev (no Docker)

```bash
bash scripts/start-qdrant.sh                       # Qdrant on :6333/:6334
cargo run --release --manifest-path src-tauri/Cargo.toml \
  --no-default-features --features web -- serve --port 8765 --ui-dir src
```

## How it's built (additive, Tauri-free)

The web binary compiles with `--no-default-features --features web`: the
`tauri`/WebKit dependencies are gated behind the default `gui` feature, so this
build links neither and runs on `debian:bookworm-slim`. The macOS app build
(`cargo build --release`, default features) is unchanged.

## Browser UI

The served UI is **functional in a browser**: a `__TAURI__` fetch shim
(injected only on the web-served `index.html`) routes the frontend's
`invoke("cmd", args)` calls to `POST /api/invoke/{cmd}`, which dispatches to the
same backend logic the desktop app uses. The Tauri desktop app is unaffected
(it loads `index.html` directly, with the real Tauri runtime).

## Hardening & self-containment

- **Non-root:** runs as user `memex` (uid 10001); ports are all > 1024.
- **Proper init:** `tini` is PID 1 (signal handling + zombie reaping) over the
  bash entrypoint that supervises Qdrant + the web server.
- **Self-contained:** the BGE-small model is **pre-baked** at build time — no
  network needed at runtime.
- **Replay & predict work:** the corpus is mounted under
  `/home/memex/.claude/projects`, a trusted `sec`-sandbox root, so `predict`
  and turn-by-turn replay (which re-parse source `.jsonl`) function in-container.

## Remaining notes

- The bundled corpus is synthetic sample data; mount your own under
  `/home/memex/.claude/projects` (+ `MEMEX_SCAN_ROOT`) for real sessions.
- `tail_recent_errors` (proactive-recall polling) returns empty in the server
  variant — a static server corpus has no live sessions changing under it.
