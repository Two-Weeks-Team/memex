# Memex · fish integration — primer on directory change.
#   source /path/to/deploy/agent-integration/shell/memex.fish   # in ~/.config/fish/config.fish
# Fail-open; human-facing.
function _memex_primer --on-variable PWD
    command -v memex >/dev/null 2>&1; or return 0
    test -d "$PWD/.git"; or test -d "$PWD/.claude"; or return 0
    if command -v timeout >/dev/null 2>&1
        timeout 2 memex memory --cwd "$PWD" --hook shell 2>/dev/null; or return 0
    else
        memex memory --cwd "$PWD" --hook shell 2>/dev/null; or return 0
    end
end
