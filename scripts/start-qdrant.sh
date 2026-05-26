#!/usr/bin/env bash
# One-command local Qdrant for Memex.
#
# Starts the pinned Qdrant container (docker-compose.yml), waits for it to
# become ready, and prints the health-check command + the URL Memex uses.
#
# Usage:
#   bash scripts/start-qdrant.sh          # start + wait for ready
#   bash scripts/start-qdrant.sh --stop   # stop the container (keeps data)
#   bash scripts/start-qdrant.sh --down   # stop + remove container (keeps volume)
#
# Exit codes:
#   0 — Qdrant is up and /readyz returned 200
#   1 — docker / docker compose not available
#   2 — Qdrant did not become ready within the timeout

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

REST_URL="http://localhost:6333"
GRPC_URL="http://localhost:6334"   # Memex connects here (MEMEX_QDRANT_URL)
SERVICE="qdrant"

if [[ -t 1 ]]; then
  G=$'\033[32m'; R=$'\033[31m'; Y=$'\033[33m'; D=$'\033[2m'; X=$'\033[0m'
else
  G=""; R=""; Y=""; D=""; X=""
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "${R}docker not found in PATH.${X} Install Docker Desktop or see docs/BUILD.md for the prebuilt-binary path." >&2
  exit 1
fi
if ! docker compose version >/dev/null 2>&1; then
  echo "${R}'docker compose' (v2) not available.${X} Update Docker, or run the binary path in docs/BUILD.md." >&2
  exit 1
fi

case "${1:-}" in
  --stop)
    docker compose stop "$SERVICE"
    echo "${Y}Qdrant stopped.${X} Data volume preserved. Restart with: bash scripts/start-qdrant.sh"
    exit 0
    ;;
  --down)
    docker compose down
    echo "${Y}Qdrant container removed.${X} The qdrant_storage volume (your indexed corpus) is kept."
    echo "Remove it too with: docker compose down -v"
    exit 0
    ;;
  "") : ;;
  *) echo "usage: $0 [--stop | --down]" >&2; exit 2 ;;
esac

echo "Starting Qdrant (qdrant/qdrant:v1.18.1) via docker compose…"
docker compose up -d "$SERVICE"

printf "Waiting for Qdrant to become ready at %s/readyz " "$REST_URL"
for _ in $(seq 1 60); do
  if curl -fsS "$REST_URL/readyz" >/dev/null 2>&1; then
    echo " ${G}ready${X}"
    echo
    echo "${G}✓ Qdrant is up.${X}"
    echo "  REST + dashboard : ${REST_URL}/dashboard"
    echo "  gRPC (Memex uses): ${GRPC_URL}   (env MEMEX_QDRANT_URL)"
    echo
    echo "${D}Health-check command:${X}"
    echo "  curl -fsS ${REST_URL}/readyz && echo OK"
    echo
    echo "Next: index a corpus, e.g."
    echo "  ./src-tauri/target/release/memex scan --path examples/sample-corpus --index"
    exit 0
  fi
  printf "."
  sleep 1
done

echo " ${R}timeout${X}" >&2
echo "Qdrant did not report ready within 60s. Inspect logs with: docker compose logs $SERVICE" >&2
exit 2
