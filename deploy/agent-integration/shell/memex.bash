# Memex · bash integration — primer on directory change (PROMPT_COMMAND last-PWD guard).
#   source /path/to/deploy/agent-integration/shell/memex.bash   # in ~/.bashrc
# Fail-open; human-facing.
_MEMEX_LAST_PWD=""
_memex_primer() {
  [ "$PWD" = "$_MEMEX_LAST_PWD" ] && return 0
  _MEMEX_LAST_PWD="$PWD"
  command -v memex >/dev/null 2>&1 || return 0
  { [ -d "$PWD/.git" ] || [ -d "$PWD/.claude" ]; } || return 0
  if command -v timeout >/dev/null 2>&1; then
    timeout 2 memex memory --cwd "$PWD" --hook shell 2>/dev/null || return 0
  else
    memex memory --cwd "$PWD" --hook shell 2>/dev/null || return 0
  fi
}
# Prepend our function once (idempotent), preserving any existing PROMPT_COMMAND.
case ";${PROMPT_COMMAND:-};" in
  *";_memex_primer;"*) ;;
  *) PROMPT_COMMAND="_memex_primer;${PROMPT_COMMAND:-}" ;;
esac
