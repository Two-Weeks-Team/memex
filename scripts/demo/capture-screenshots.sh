#!/usr/bin/env bash
# P8 — Empirically validate the `memex://` deep-link plugin by issuing
# the 5 routes via macOS `open` and capturing screenshots for evidence.
#
# Requirements:
#   - Memex.app installed (DMG mounted + dragged to Applications) OR
#     run with $MEMEX_APP_PATH pointing to the .app bundle
#   - macOS Accessibility permission for screencapture (auto-prompted)
#   - $MEMEX_QDRANT_HTTP_URL reachable + v3 collection populated
#
# Usage:
#   bash scripts/demo/capture-screenshots.sh            # all 5 routes
#   bash scripts/demo/capture-screenshots.sh topology   # one route
#
# Output:
#   tests/e2e/screenshots/{route}.png — one PNG per surface

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# ---- color helpers -------------------------------------------------------
if [[ -t 1 ]]; then
  R=$'\033[31m'; G=$'\033[32m'; Y=$'\033[33m'; B=$'\033[34m'; D=$'\033[2m'; X=$'\033[0m'
else
  R=""; G=""; Y=""; B=""; D=""; X=""
fi
step() { printf "${B}┌─ %s${X}\n" "$1"; }
ok()   { printf "  ${G}✓${X} %s\n" "$1"; }
fail() { printf "  ${R}✗${X} %s\n" "$1" >&2; }
info() { printf "  ${D}%s${X}\n" "$1"; }

OUTDIR="$REPO_ROOT/tests/e2e/screenshots"
mkdir -p "$OUTDIR"

# ---- locate Memex.app ----------------------------------------------------
locate_app() {
  if [[ -n "${MEMEX_APP_PATH:-}" ]] && [[ -d "$MEMEX_APP_PATH" ]]; then
    echo "$MEMEX_APP_PATH"
    return 0
  fi
  for candidate in \
    "/Applications/Memex.app" \
    "$HOME/Applications/Memex.app" \
    "$REPO_ROOT/src-tauri/target/release/bundle/macos/Memex.app"; do
    if [[ -d "$candidate" ]]; then
      echo "$candidate"
      return 0
    fi
  done
  return 1
}

APP_PATH=$(locate_app) || {
  fail "Memex.app not found — pass MEMEX_APP_PATH or run npm run tauri build"
  exit 1
}
step "Memex.app: $APP_PATH"

# ---- start the app if not running ---------------------------------------
ensure_running() {
  if pgrep -x memex >/dev/null || pgrep -i Memex >/dev/null; then
    ok "Memex already running"
    return 0
  fi
  ok "launching Memex"
  open "$APP_PATH"
  # Wait for the window to appear (process must exist).
  local tries=0
  until pgrep -i Memex >/dev/null; do
    sleep 1
    tries=$((tries+1))
    if (( tries > 30 )); then
      fail "Memex did not start within 30s"
      return 1
    fi
  done
  # First-run can be slow (lazy AppState init); give the webview a beat.
  sleep 4
  return 0
}

# ---- screenshot helper ---------------------------------------------------
capture_route() {
  local route="$1"
  local outfile="$OUTDIR/$route.png"
  step "Route: memex://$route"
  # ROBUSTNESS FIX (Gemini PR #10 review, capture-screenshots.sh:93): the
  # AppleScript `tell process "Memex"` is case-sensitive. On a dev build
  # ran straight from `cargo run` (no `.app` bundle, e.g. CI matrix) the
  # process name is the lowercase binary name `memex`, and the script
  # silently swallowed the Escape keystroke. Resolve the actual process
  # name via the running PID and feed it into the osascript so we work
  # in both bundled and dev modes.
  local proc_name="Memex"
  local pid
  pid=$(pgrep -i Memex 2>/dev/null | head -1 || true)
  if [[ -n "$pid" ]]; then
    local resolved
    resolved=$(ps -p "$pid" -o comm= 2>/dev/null | xargs -I {} basename {} || true)
    if [[ -n "$resolved" ]]; then
      proc_name="$resolved"
    fi
  fi
  # Reset state between routes — press Escape to close any open dialog so
  # each screenshot captures the surface the deep link actually opens, not
  # the previous route's modal stacked underneath.
  osascript -e "tell application \"System Events\"
    tell process \"$proc_name\"
      try
        key code 53 -- Escape closes <dialog>
      end try
    end tell
  end tell" 2>/dev/null || true
  sleep 0.5
  # `open` invokes LaunchServices which routes the URL scheme to the .app.
  # If the app was just launched, the cold-start `get_current` path fires
  # the route. Subsequent calls fire the `deep-link://new-url` event.
  open "memex://$route"
  # Let the surface render. Different surfaces need different settle times.
  case "$route" in
    topology|mix-match) sleep 3 ;;
    predict)            sleep 2 ;;
    *)                  sleep 1 ;;
  esac
  # Bring Memex window unambiguously to the absolute front. The combination
  # of `activate` + raising the System Events process window covers both
  # the LaunchServices and tray-icon code paths. Uses the dynamically
  # resolved process name (see proc_name above) so a lowercase dev build
  # still works.
  osascript -e "
tell application \"Memex\" to activate
tell application \"System Events\"
  set frontmost of process \"$proc_name\" to true
end tell
" 2>/dev/null || true
  sleep 0.8
  # Capture the Memex window's bounds directly via System Events, then
  # screencapture the rectangle. This avoids the `screencapture -l` API
  # which silently falls back to full-screen on macOS 14+.
  local bounds
  bounds=$(osascript -e "
tell application \"System Events\"
  tell process \"$proc_name\"
    set p to position of window 1
    set s to size of window 1
    return ((item 1 of p) as string) & \",\" & ((item 2 of p) as string) & \",\" & ((item 1 of s) as string) & \",\" & ((item 2 of s) as string)
  end tell
end tell
" 2>/dev/null || echo "")
  if [[ -n "$bounds" ]] && [[ "$bounds" == *","* ]]; then
    # screencapture -R x,y,w,h captures that region without the surrounding
    # workspace clutter. We don't care about retina scaling — screencapture
    # already produces 2x pixels on retina by default.
    if ! screencapture -x -o -R "$bounds" "$outfile"; then
      screencapture -x -o -T 0 "$outfile" || return 1
    fi
  else
    if ! screencapture -x -o -T 0 "$outfile"; then
      fail "screencapture failed for $route"
      return 1
    fi
  fi
  if [[ ! -s "$outfile" ]]; then
    fail "screenshot is empty: $outfile"
    return 1
  fi
  ok "wrote $outfile ($(du -h "$outfile" | cut -f1))"
  return 0
}

# ---- main ----------------------------------------------------------------
ensure_running || exit 1

# Allow filtering to one route as a positional argument.
ROUTES=(timemachine topology lens predict mix-match)
if [[ $# -gt 0 ]]; then
  ROUTES=("$@")
fi

FAILED=0
for r in "${ROUTES[@]}"; do
  capture_route "$r" || FAILED=$((FAILED+1))
done

step "Summary"
COUNT=$(find "$OUTDIR" -name '*.png' -type f | wc -l | tr -d ' ')
if (( FAILED == 0 )) && (( COUNT >= 5 )); then
  printf "  ${G}✓ ${COUNT} screenshot(s) captured${X}\n"
  exit 0
else
  printf "  ${R}✗ ${FAILED} route(s) failed; ${COUNT} screenshot(s) on disk${X}\n"
  exit 2
fi
