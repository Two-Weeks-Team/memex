#!/usr/bin/env bash
# Memex · Claude Code SessionStart hook — inject the Companion primer into model context.
#
# Installed (NOT committed as active config) into .claude/settings.local.json by
# `memex install --hooks`. See THR-01 in claudedocs/reports/pr8/security-threat-model.md
# for why these hooks are local-installed rather than committed to the repo.
#
# Fail-open by construction: any missing binary / timeout / error exits 0 with no
# stdout, so a slow or dead engine NEVER blocks the session.
set -u

command -v memex >/dev/null 2>&1 || exit 0
dir="${CLAUDE_PROJECT_DIR:-$PWD}"

# Bounded wrapper (belt-and-suspenders over memex's own connect timeout).
run() {
  if command -v timeout >/dev/null 2>&1; then timeout "$1" "${@:2}"
  elif command -v gtimeout >/dev/null 2>&1; then gtimeout "$1" "${@:2}"
  else shift; "$@"; fi
}

# `--hook session-start` emits ready-to-use Claude Code hook JSON
#   {"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"..."}}
# sanitized and budgeted to ~200-500 tokens of factual context (no jq needed here).
run 2 memex memory --cwd "$dir" --hook session-start 2>/dev/null || exit 0
