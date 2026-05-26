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

> First index/search downloads the BGE-small embedding model (~130 MB) from
> Hugging Face into `/app/cache` — the container needs network on first use.
> Mount a volume at `/app/cache` to cache it across runs.

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

## Notes / limitations

- The static UI (`src/`) currently targets Tauri IPC; over HTTP it loads and
  renders the shell, while the **JSON API** is the programmatic surface. Wiring
  the UI to `fetch` the JSON API is a follow-up.
- The container runs Qdrant + the web server under a small bash supervisor
  (entrypoint). A process supervisor (tini/s6) would be sturdier for production.
- Runs as root inside the container (MVP). Add a non-root `USER` for hardening.
