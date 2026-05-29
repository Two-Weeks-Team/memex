#!/usr/bin/env bash
# Single-container entrypoint: start Qdrant (internal), wait for it, then run
# the memex web service in the foreground. Both live in this one image.
set -euo pipefail

STORAGE="${QDRANT__STORAGE__STORAGE_PATH:-/qdrant/storage}"
PORT="${MEMEX_WEB_PORT:-8765}"
UI_DIR="${MEMEX_WEB_UI_DIR:-/app/ui}"
mkdir -p "$STORAGE" "${XDG_CACHE_HOME:-/app/cache}"

echo "[entrypoint] starting Qdrant 1.18.1 (internal REST :6333 / gRPC :6334)…"
qdrant &
QPID=$!
trap 'echo "[entrypoint] stopping Qdrant…"; kill "$QPID" 2>/dev/null || true' EXIT INT TERM

echo -n "[entrypoint] waiting for Qdrant /readyz "
for _ in $(seq 1 90); do
  if curl -fsS http://localhost:6333/readyz >/dev/null 2>&1; then
    echo " ready"
    break
  fi
  printf "."
  sleep 1
done
if ! curl -fsS http://localhost:6333/readyz >/dev/null 2>&1; then
  echo " FAILED — Qdrant did not become ready" >&2
  exit 1
fi

export MEMEX_QDRANT_URL="${MEMEX_QDRANT_URL:-http://localhost:6334}"
echo "[entrypoint] starting memex web on :${PORT} (UI: ${UI_DIR}, MCP: /mcp)…"
# Foreground; the EXIT trap tears Qdrant down when this returns.
memex serve --port "$PORT" --ui-dir "$UI_DIR"
