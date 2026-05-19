#!/usr/bin/env bash
# Memex E2E smoke test (P8) — empirical validation against real corpus
#
# Exits 0 only if ALL 7 Memex surfaces respond non-empty on the live Qdrant
# `memex_sessions_v3` collection. Designed to be idempotent and CI-runnable.
#
# Prerequisites (verified by --check before main run):
#   - docker container `memex-qdrant` Up on :6333 / :6334
#   - collection `memex_sessions_v3` populated (run `memex scan --index`)
#   - `./src-tauri/target/release/memex` built
#   - jq + curl in PATH
#
# Usage:
#   bash scripts/demo/smoke-test.sh           # full run
#   bash scripts/demo/smoke-test.sh --check   # only environment preflight
#   bash scripts/demo/smoke-test.sh --json    # full run; writes per-surface
#                                             # JSON to tests/e2e/*.json
#
# Exit codes:
#   0 — all surfaces responded non-empty
#   1 — environment failure (Qdrant down, binary missing, jq missing)
#   2 — surface returned empty/error
#   3 — collection has insufficient data (< 80 points)

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# ---- styling -------------------------------------------------------------
if [[ -t 1 ]]; then
  R=$'\033[31m'; G=$'\033[32m'; Y=$'\033[33m'; B=$'\033[34m'; D=$'\033[2m'; X=$'\033[0m'
else
  R=""; G=""; Y=""; B=""; D=""; X=""
fi
step() { printf "${B}┌─ %s${X}\n" "$1"; }
ok()   { printf "  ${G}✓${X} %s\n" "$1"; }
fail() { printf "  ${R}✗${X} %s\n" "$1" >&2; }
info() { printf "  ${D}%s${X}\n" "$1"; }

# ---- config --------------------------------------------------------------
QDRANT_HTTP="${MEMEX_QDRANT_HTTP_URL:-http://localhost:6333}"
COLLECTION="memex_sessions_v3"
MIN_POINTS=80
MEMEX_BIN="$REPO_ROOT/src-tauri/target/release/memex"
EVIDENCE_DIR="$REPO_ROOT/tests/e2e"
WRITE_JSON=false

case "${1:-}" in
  --check) ONLY_CHECK=true ;;
  --json)  WRITE_JSON=true; ONLY_CHECK=false ;;
  "")      ONLY_CHECK=false ;;
  *) echo "usage: $0 [--check | --json]" >&2; exit 2 ;;
esac

# ---- preflight -----------------------------------------------------------
preflight() {
  step "Preflight (Qdrant · binary · jq)"
  if ! command -v jq >/dev/null 2>&1; then fail "jq not in PATH"; return 1; fi
  ok "jq available"
  if ! command -v curl >/dev/null 2>&1; then fail "curl not in PATH"; return 1; fi
  ok "curl available"
  if ! curl -sf "$QDRANT_HTTP/readyz" >/dev/null; then
    fail "Qdrant not ready at $QDRANT_HTTP/readyz"; return 1
  fi
  ok "Qdrant /readyz OK"
  if [[ ! -x "$MEMEX_BIN" ]]; then
    fail "memex binary missing: $MEMEX_BIN — run cargo build --release"; return 1
  fi
  ok "memex binary present"
  return 0
}

# ---- collection check ----------------------------------------------------
check_collection() {
  step "Collection (must have >= $MIN_POINTS points)"
  local info
  info=$(curl -s "$QDRANT_HTTP/collections/$COLLECTION" 2>&1) || {
    fail "GET collection failed"; return 3
  }
  local status points
  status=$(echo "$info" | jq -r '.status // "?"')
  if [[ "$status" != "ok" ]]; then
    fail "collection status: $status — run: $MEMEX_BIN scan --index"
    info "raw: $(echo "$info" | head -c 200)"
    return 3
  fi
  points=$(echo "$info" | jq -r '.result.points_count // 0')
  if (( points < MIN_POINTS )); then
    fail "points_count=$points < $MIN_POINTS — re-run scan --index"
    return 3
  fi
  ok "points_count=$points (>= $MIN_POINTS)"
  if [[ "$WRITE_JSON" == true ]]; then
    mkdir -p "$EVIDENCE_DIR"
    echo "$info" > "$EVIDENCE_DIR/collection-info.json"
    ok "wrote tests/e2e/collection-info.json"
  fi
  return 0
}

# ---- surface harness -----------------------------------------------------
# Run a memex CLI surface, verify it returns non-empty content. Writes the
# raw stdout to a file under `$EVIDENCE_DIR` for later inspection.
#
# EXTENSION FIX (Gemini PR #10 review, smoke-test.sh:108): most CLI surfaces
# (scan, search, lens, recall, predict, mix) emit a human-readable table
# header + rows, not JSON. Writing those into `.json` files misled
# downstream automation that tried to `jq` them. Pick the extension based
# on what the surface actually returns: `.json` only when stdout starts
# with `{` or `[` (topology, collection-info), `.txt` otherwise.
run_surface() {
  local name="$1"; shift
  local desc="$1"; shift
  step "Surface: $name ($desc)"
  info "cmd: $MEMEX_BIN $*"
  local out
  if ! out=$("$MEMEX_BIN" "$@" 2>&1); then
    fail "$name: command failed (exit non-zero)"
    info "out (first 500 chars): $(echo "$out" | head -c 500)"
    return 2
  fi
  if [[ -z "$out" ]]; then
    fail "$name: empty stdout"
    return 2
  fi
  # Heuristic: at least 30 chars or contains '{' or '['
  local len=${#out}
  if (( len < 30 )) && [[ "$out" != *'{'* && "$out" != *'['* ]]; then
    fail "$name: output too short ($len chars): $out"
    return 2
  fi
  if [[ "$WRITE_JSON" == true ]]; then
    mkdir -p "$EVIDENCE_DIR"
    # Sniff the first non-whitespace byte to pick the file extension.
    local first
    first=$(printf '%s' "$out" | sed -e 's/^[[:space:]]*//' | cut -c1)
    local ext="txt"
    if [[ "$first" == "{" || "$first" == "[" ]]; then
      ext="json"
    fi
    local outfile="$EVIDENCE_DIR/$name.$ext"
    echo "$out" > "$outfile"
    ok "wrote $outfile (${len} bytes)"
  else
    ok "non-empty output (${len} bytes)"
  fi
  return 0
}

# ---- representative session id (used for predict/mix) -------------------
# Scrolls the collection and prints first point id (UUID).
pick_session_ids() {
  local out
  # v3 payload uses snake_case `session_id` (the original UUID from the
  # source JSONL — predict/mix need this, NOT the Qdrant point id, which
  # is a UUIDv5 hash of it).
  out=$(curl -s "$QDRANT_HTTP/collections/$COLLECTION/points/scroll" \
        -H 'Content-Type: application/json' \
        -d '{"limit":4,"with_payload":["session_id"],"with_vector":false}') || return 1
  echo "$out" | jq -r '.result.points[].payload.session_id' 2>/dev/null
}

# ---- main ----------------------------------------------------------------
preflight || exit 1
check_collection || exit $?

if [[ "${ONLY_CHECK:-false}" == true ]]; then
  printf "${G}preflight OK — exiting (--check)${X}\n"
  exit 0
fi

mkdir -p "$EVIDENCE_DIR"

SID_A=""
SID_B=""
while IFS= read -r line; do
  if [[ -z "$SID_A" ]]; then SID_A="$line"
  elif [[ -z "$SID_B" ]]; then SID_B="$line"; fi
done < <(pick_session_ids | head -2)
SID_B="${SID_B:-$SID_A}"
if [[ -z "$SID_A" ]]; then
  fail "could not retrieve a session id from collection"; exit 2
fi
ok "sample session id A=$SID_A"
ok "sample session id B=$SID_B"
echo "$SID_A" > "$EVIDENCE_DIR/sample-session-id.txt"

FAILED=0

# 1) scan (parse only, no re-index)
run_surface scan "session parse" scan --limit 5 || FAILED=$((FAILED+1))

# 2) search (dense KNN on `content`)
run_surface search "vector KNN" search "edit auth.js" || FAILED=$((FAILED+1))

# 3) lens (P2 FormulaQuery; weighted multi-vector)
run_surface lens "FormulaQuery lens" lens "edit auth.js" || FAILED=$((FAILED+1))

# 4) topology (MST graph)
# STALE-OUTPUT FIX (Gemini PR #10 review, smoke-test.sh:187): remove any
# leftover /tmp/topology.json from a previous run first, so a NEW failure
# of `memex topology --out` doesn't silently re-use the prior good output
# (the file-exists check below was happy to validate a stale snapshot).
rm -f /tmp/topology.json
run_surface topology "MST topology" topology --sample 30 --out /tmp/topology.json || FAILED=$((FAILED+1))
if [[ -f /tmp/topology.json ]]; then
  nodes=$(jq '(.nodes // []) | length' /tmp/topology.json 2>/dev/null || echo 0)
  if (( nodes >= 30 )); then
    ok "topology nodes=$nodes"
    cp /tmp/topology.json "$EVIDENCE_DIR/topology.json"
  else
    fail "topology nodes=$nodes < 30"
    FAILED=$((FAILED+1))
  fi
fi

# 5) recall (proactive past-error match)
run_surface recall "proactive recall" recall "cargo build error" || FAILED=$((FAILED+1))

# 6) predict (next-action mining)
run_surface predict "next-action predict" predict "$SID_A" --neighbors 5 || FAILED=$((FAILED+1))

# 7) mix (Discovery API)
run_surface mix "Discovery mix" mix --pos "$SID_A" --neg "$SID_B" || FAILED=$((FAILED+1))

step "Summary"
if (( FAILED == 0 )); then
  printf "  ${G}✓ all 7 surfaces returned non-empty${X}\n"
  exit 0
else
  printf "  ${R}✗ $FAILED surface(s) failed${X}\n"
  exit 2
fi
