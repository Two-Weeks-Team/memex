#!/usr/bin/env bash
# Memex · Claude Code PostToolUse hook (matcher: Bash) — Loop Breaker.
#
# Claude Code has NO "error matcher" event — PostToolUse fires after a tool SUCCEEDS,
# and the matcher only matches tool NAMES. So we match `Bash`, then inspect the
# tool_response delivered on stdin: memex detects the stuck pattern (>= error
# threshold in the recent window) and, only then, emits a pivot suggestion via
# additionalContext. The hook cannot re-run the tool — it informs, Claude decides.
# Fail-open; success/no-error case emits nothing (true-negative).
set -u

command -v memex >/dev/null 2>&1 || exit 0
dir="${CLAUDE_PROJECT_DIR:-$PWD}"

run() {
  if command -v timeout >/dev/null 2>&1; then timeout "$1" "${@:2}"
  elif command -v gtimeout >/dev/null 2>&1; then gtimeout "$1" "${@:2}"
  else shift; "$@"; fi
}

# stdin = PostToolUse JSON (tool_name, tool_input, tool_response). memex parses the
# error signal, applies the LOOP_* thresholds (lifted into the headless loopcheck
# module), and emits a pivot or nothing.
run 1 memex loop-check --cwd "$dir" --hook post-tool-use 2>/dev/null || exit 0
