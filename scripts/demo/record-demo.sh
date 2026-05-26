#!/usr/bin/env bash
# Memex VSD 2026 demo recording helper
#
# This script does NOT capture video itself — operator uses OBS for that
# (more reliable on macOS 15+ with hardware encoding). Instead it:
#   1. Verifies the runtime environment (Qdrant, fastembed cache, corpus)
#   2. Pre-stages clipboard with the demo queries
#   3. Plays an audible cue every 5s during recording so the operator
#      can sync shots against the 18-shot timeline in
#      claudedocs/phases/phase-7-demo-production/video-script.md
#
# Usage:
#   scripts/demo/record-demo.sh            # full pre-flight + cue track
#   scripts/demo/record-demo.sh --dry-run  # only pre-flight checks
#   scripts/demo/record-demo.sh --cues     # only the cue track (assumes
#                                          # operator already ran OBS)
#   scripts/demo/record-demo.sh --post FILE.mov   # post-process recording
#                                                  # to 24fps mp4 via ffmpeg

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT"

# ---- colors --------------------------------------------------------------
if [[ -t 1 ]]; then
  R=$'\033[31m'; G=$'\033[32m'; Y=$'\033[33m'; B=$'\033[34m'; D=$'\033[2m'; X=$'\033[0m'
else
  R=""; G=""; Y=""; B=""; D=""; X=""
fi

step() { printf "${B}┌─ %s${X}\n" "$1"; }
ok()   { printf "  ${G}✓${X} %s\n" "$1"; }
warn() { printf "  ${Y}⚠${X} %s\n" "$1"; }
err()  { printf "  ${R}✗${X} %s\n" "$1"; }
info() { printf "  ${D}%s${X}\n" "$1"; }

# ---- pre-flight ---------------------------------------------------------
preflight() {
  step "Pre-flight (Qdrant · fastembed · corpus)"

  # ROBUSTNESS (Gemini PR #8 review, record-demo.sh:45): check ALL the
  # commands we actually use, not just docker. The Qdrant version probe
  # below (line ~63) needs curl + python3, and if either is missing we'd
  # silently get a "?" version and skip the check. Fail fast with a clear
  # message instead.
  for cmd in docker curl python3; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
      err "$cmd not in PATH"
      return 1
    fi
  done

  if docker ps --filter 'name=memex-qdrant' --format '{{.Status}}' | grep -q '^Up'; then
    ok "Qdrant container memex-qdrant Up"
  else
    err "memex-qdrant container not running"
    info "fix: docker run -d --name memex-qdrant -p 6333:6333 -p 6334:6334 qdrant/qdrant:v1.18.1"
    return 1
  fi

  if curl -sf http://localhost:6333/readyz >/dev/null; then
    ok "Qdrant /readyz: all shards are ready"
  else
    err "Qdrant /readyz unhealthy"
    return 1
  fi

  local ver
  ver=$(curl -s http://localhost:6333 | python3 -c "import sys,json;print(json.load(sys.stdin).get('version','?'))" 2>/dev/null || echo "?")
  if [[ "$ver" == 1.18.* ]]; then
    ok "Qdrant version $ver confirmed (1.18.x)"
  else
    warn "Qdrant version is $ver (expected 1.18.x)"
  fi

  if [[ -d "$HOME/.fastembed_cache" ]] && [[ -n "$(ls -A "$HOME/.fastembed_cache" 2>/dev/null)" ]]; then
    ok "fastembed cache present"
  else
    warn "fastembed cache empty — first scan will download ~130MB"
  fi

  if [[ -d "$HOME/.claude/projects" ]]; then
    local claude_count
    claude_count=$(find "$HOME/.claude/projects" -maxdepth 3 -name "*.jsonl" 2>/dev/null | wc -l | tr -d ' ')
    ok "Claude corpus: $claude_count session files in ~/.claude/projects"
  else
    warn "~/.claude/projects missing"
  fi

  if [[ -d "$HOME/.codex/sessions" ]]; then
    local codex_count
    codex_count=$(find "$HOME/.codex/sessions" -name "rollout-*.jsonl" 2>/dev/null | wc -l | tr -d ' ')
    ok "Codex corpus: $codex_count session files in ~/.codex/sessions"
  else
    warn "~/.codex/sessions missing — KH-01 multi-agent demo will be Claude-only"
  fi

  # ROBUSTNESS (Codex PR #8 review, record-demo.sh:93): with `set -euo
  # pipefail`, a missing target/release/bundle/dmg directory makes the
  # `find` pipe exit non-zero and aborts the entire script before we can
  # print the "no DMG bundle" warning. Guard the existence of the parent
  # directory first; on a fresh checkout (no tauri build yet) we now emit
  # the warning and continue rather than aborting --dry-run.
  local dmg=""
  if [[ -d src-tauri/target/release/bundle/dmg ]]; then
    dmg=$(find src-tauri/target/release/bundle/dmg -name "Memex_*.dmg" 2>/dev/null | head -1 || true)
  fi
  if [[ -n "$dmg" ]]; then
    ok "DMG built: $dmg ($(du -h "$dmg" | cut -f1))"
  else
    warn "no DMG bundle — run: npm run tauri build"
  fi
}

# ---- pre-stage clipboard ------------------------------------------------
stage_clipboard() {
  step "Pre-stage clipboard for Act II/III queries"
  if ! command -v pbcopy >/dev/null 2>&1; then
    warn "pbcopy not available — manual copy required"
    return 0
  fi
  printf "edit auth.js" | pbcopy
  ok "Clipboard: 'edit auth.js' (Act II at 1:10)"
  info "After Act II, copy the cargo build error string for Act III (1:54)"
}

# ---- cue track ----------------------------------------------------------
# The cue track prints a marker every 5s with the active shot from the
# script. Operator follows along while OBS records.
cue_track() {
  step "Cue track — 18 shots over 180s"
  info "press ENTER to start recording timer (sync to OBS hot-key)"
  read -r _

  local -a cues=(
    "0:00  COLD OPEN  · Bush quote fade-in (silence)"
    "0:08  Act I     · Time Machine stack fly-in (music drops)"
    "0:18  Act I     · enrich.rs chip close-up (chime)"
    "0:28  Act I     · '80 sessions · 17938 tool calls' count-up"
    "0:40  Act I     · ⌘+T → Topology galaxy with cluster labels"
    "0:55  Act I     · Bridge edge + gap-insight bubble"
    "1:10  Act II    · ⌘K · type 'edit auth.js' · split-screen 5→1 rt"
    "1:22  Act II    · contribution bars animate (breakdown chip)"
    "1:32  Act II    · Predict 4×3 thumbnails · View Transition zoom"
    "1:42  CLIMAX    · cargo build fail · MUSIC FADE · SILENCE 12s"
    "1:54  CLIMAX    · Recall banner slides in (soft banner SFX)"
    "2:00  Act IV    · Music returns slow · cut to Mix & Match"
    "2:08  Act IV    · drag pos+neg · 3D hyperplane materializes"
    "2:22  Act IV    · 👍 click · cards re-rank (chime)"
    "2:35  Act V     · Topology agent filter [Claude→Codex→Both]"
    "2:48  OUTRO     · Title card (Memex · spatial memory · 1.18 pinnacle)"
    "2:56  OUTRO     · License row + 'Think Outside the Bot ✓'"
    "3:00  END"
  )

  local -a cues_sec=(0 8 18 28 40 55 70 82 92 102 114 120 128 142 155 168 176 180)

  local t0
  t0=$(date +%s)

  for i in "${!cues[@]}"; do
    local target=${cues_sec[$i]}
    local now elapsed wait_for
    while :; do
      now=$(date +%s)
      elapsed=$(( now - t0 ))
      if (( elapsed >= target )); then break; fi
      sleep 0.2
    done
    printf "${G}▶${X} %s\n" "${cues[$i]}"
    # Bell at start (1:42 = 102s) AND end (1:54 = 114s) of the 12s
    # climax silence. Gemini PR #8 review: the operator needs the second
    # bell to know when to resume Act III action (Recall banner slide-in)
    # without watching the clock while OBS is recording.
    if (( target == 102 || target == 114 )); then printf "\a"; fi
  done

  printf "${B}─── recording window closed (3:00 elapsed) ───${X}\n"
}

# ---- post-process to 24fps mp4 ------------------------------------------
postprocess() {
  local infile="$1"
  if [[ ! -f "$infile" ]]; then
    err "input file not found: $infile"
    return 1
  fi
  if ! command -v ffmpeg >/dev/null 2>&1; then
    err "ffmpeg not in PATH"
    info "brew install ffmpeg"
    return 1
  fi
  local outfile="${infile%.*}-24fps.mp4"
  step "Post-process: $infile → $outfile (60fps→24fps, h264, AAC 192k)"
  ffmpeg -y -i "$infile" \
    -r 24 \
    -c:v libx264 -preset slow -crf 18 -pix_fmt yuv420p \
    -c:a aac -b:a 192k \
    -movflags +faststart \
    "$outfile"
  ok "Done: $outfile"
  info "Upload to YouTube unlisted (per AC-7.4.2)"
}

# ---- main ---------------------------------------------------------------
case "${1:-}" in
  --dry-run)
    preflight
    ;;
  --cues)
    cue_track
    ;;
  --post)
    postprocess "${2:?--post requires a file path}"
    ;;
  "")
    preflight
    stage_clipboard
    cue_track
    ;;
  *)
    echo "usage: $0 [--dry-run | --cues | --post FILE.mov]" >&2
    exit 2
    ;;
esac
