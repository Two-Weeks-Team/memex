#!/usr/bin/env bash
# Memex · Claude Code SessionEnd hook — reindex the just-finished session for freshness.
#
# Deliberately SessionEnd, NOT Stop: `Stop` fires after EVERY agent turn (not at
# session end), and a blocking Stop-style hook once burned a full session in an
# infinite loop. SessionEnd fires once and needs no context injection — ideal for
# reindex. Detached + instant exit 0 so it never adds latency; memex reindex
# self-debounces and is incremental/idempotent.
set -u

command -v memex >/dev/null 2>&1 || exit 0
dir="${CLAUDE_PROJECT_DIR:-$PWD}"

# Detach with nohup + disown so reindex escapes the hook's process group and survives
# a timeout SIGTERM; the hook still returns instantly. memex reindex self-debounces and
# is incremental/idempotent. (`SessionEnd` is a confirmed Claude Code hook event; it
# cannot inject context, which is fine — reindex needs none.)
nohup memex reindex --cwd "$dir" --hook session-end >/dev/null 2>&1 &
disown "$!" 2>/dev/null || true
exit 0
