# Memex · zsh integration — print the Companion primer when you cd into a project.
#   source /path/to/deploy/agent-integration/shell/memex.zsh   # in ~/.zshrc
#
# Host-agnostic: works alongside Claude Code / Codex / a bare terminal. Human-facing
# (prints to the terminal, not into an agent's context). Fail-open.
_memex_primer() {
  command -v memex >/dev/null 2>&1 || return 0
  local dir="${PWD}"
  [[ -d "$dir/.git" || -d "$dir/.claude" ]] || return 0   # only inside a project
  # `--hook shell` = plain markdown, ANSI-stripped (THR-07); memex rate-limits per cwd.
  if command -v timeout >/dev/null 2>&1; then
    timeout 2 memex memory --cwd "$dir" --hook shell 2>/dev/null || return 0
  else
    memex memory --cwd "$dir" --hook shell 2>/dev/null || return 0
  fi
}
# Append (don't clobber) to chpwd hooks; chpwd does NOT fire at startup, so prime once.
typeset -ga chpwd_functions
chpwd_functions+=(_memex_primer)
_memex_primer
