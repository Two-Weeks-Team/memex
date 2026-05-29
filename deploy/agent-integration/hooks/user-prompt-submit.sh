#!/usr/bin/env bash
# Memex · Claude Code UserPromptSubmit hook — relevance-gated recall injection.
#
# Fires on every prompt (30s hard cap in Claude Code; we target <1s). Reads the
# UserPromptSubmit JSON (incl. .prompt) on stdin; memex relevance-gates it and emits
# additionalContext ONLY when a past-session match clears the threshold — otherwise
# nothing (silence is correct; over-injection is the #1 reason users disable this).
# Fail-open.
set -u

command -v memex >/dev/null 2>&1 || exit 0
dir="${CLAUDE_PROJECT_DIR:-$PWD}"

run() {
  if command -v timeout >/dev/null 2>&1; then timeout "$1" "${@:2}"
  elif command -v gtimeout >/dev/null 2>&1; then gtimeout "$1" "${@:2}"
  else shift; "$@"; fi
}

# stdin (the hook JSON) is passed through to memex, which parses .prompt, runs the
# relevance gate, and emits hook JSON with additionalContext (or empty → no inject).
run 1 memex recall --cwd "$dir" --hook user-prompt-submit 2>/dev/null || exit 0
