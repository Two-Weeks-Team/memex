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
  elif command -v gtimeout >/dev/null 2>&1; then
    gtimeout 2 memex memory --cwd "$PWD" --hook shell 2>/dev/null || return 0
  else
    memex memory --cwd "$PWD" --hook shell 2>/dev/null || return 0
  fi
}
# Register our function once (idempotent), preserving any existing PROMPT_COMMAND.
# Bash 5.1+ allows PROMPT_COMMAND to be an array; a string assignment would
# destroy it (and any other registered hooks: starship, autojump, venv, …).
if declare -p PROMPT_COMMAND 2>/dev/null | grep -q 'declare -a'; then
  case " ${PROMPT_COMMAND[*]} " in
    *" _memex_primer "*) ;;
    *) PROMPT_COMMAND=(_memex_primer "${PROMPT_COMMAND[@]}") ;;
  esac
else
  case ";${PROMPT_COMMAND:-};" in
    *";_memex_primer;"*) ;;
    *) PROMPT_COMMAND="_memex_primer;${PROMPT_COMMAND:-}" ;;
  esac
fi
